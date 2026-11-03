<template>
  <div class="ct-page">
    <!-- 顶部操作栏 -->
    <el-card shadow="never" class="ct-header-card">
      <div class="ct-header">
        <div class="ct-title">
          <el-icon class="ct-title-icon"><Connection /></el-icon>
          <div>
            <h3>连接测试中心</h3>
            <p>统一检测设备采集链路与上行通道的连通性，测试前请确保网关配置已保存。</p>
          </div>
        </div>
        <div class="ct-actions">
          <el-button :icon="Refresh" @click="loadAll" :loading="loading">刷新</el-button>
          <el-button type="primary" :icon="VideoPlay" @click="testAll" :loading="testing" :disabled="!hasItems">
            {{ testing ? '测试中...' : '一键测试全部' }}
          </el-button>
        </div>
      </div>

      <!-- 汇总统计 -->
      <TestSummaryPanel :summary="summary" />
    </el-card>

    <!-- 设备连接测试 -->
    <DeviceTestForm
      :devices="devices"
      :testing="testingDevices"
      @test-one="testDevice"
      @test-all="testAllDevices"
    />

    <!-- 上行通道连接测试 -->
    <ChannelTestForm
      :channels="channels"
      :testing="testingChannels"
      @test-one="testChannel"
      @test-all="testAllChannels"
    />

    <!-- A6：异常码速查并入连接测试。测试失败时现场最需要「这个码是什么意思、怎么处理」，
         原独立页签会把这一步拆成两次跳转；这里折成可展开的参考区，同一屏对照 -->
    <el-card shadow="never" class="ct-error-card">
      <el-collapse v-model="errorPanels">
        <el-collapse-item name="errors" title="异常码速查：测试失败时对照码表看含义、原因与处理建议">
          <ModbusError />
        </el-collapse-item>
      </el-collapse>
    </el-card>
  </div>
</template>

<script setup lang="ts">
/**
 * ConnectionTest — 连接测试中心（容器组件）
 *
 * S4 重构后仅负责：加载设备/通道数据、执行测试逻辑、维护共享状态，
 * 渲染拆分为三个展示型子组件：
 *   - DeviceTestForm：设备采集链路表格 + 状态标签
 *   - ChannelTestForm：上行通道表格 + 状态标签
 *   - TestSummaryPanel：通过/失败/平均耗时汇总
 *
 * 子组件通过 emit 上抛 test-one / test-all 事件，容器在此调用对应测试函数；
 * 测试状态字段（_status/_latency/_error/_detail）写在行对象上，响应式驱动子组件刷新。
 */
import { ref, computed, onMounted } from 'vue'
import { Connection, Refresh, VideoPlay } from '@element-plus/icons-vue'
import { ElMessage } from 'element-plus'
import { listDevices } from '@/api/devices'
import { listPoints, readPointNow } from '@/api/points'
import { getNorthStatus, type NorthStatus } from '@/api/uplink'
import { getUplinkConfig, testUplink, type UplinkConfig } from '@/api/uplink'
import DeviceTestForm from './components/DeviceTestForm.vue'
import ChannelTestForm from './components/ChannelTestForm.vue'
import TestSummaryPanel from './components/TestSummaryPanel.vue'
import ModbusError from '../bus/ModbusError.vue'
import type { DeviceRow, ChannelRow, TestSummary } from './components/types'
import { extractErrorMessage } from '@/utils/errorHandler'
import { formatClock } from '@/utils/datetime'

const devices = ref<DeviceRow[]>([])
const channels = ref<ChannelRow[]>([])
const loading = ref(false)
const testing = ref(false)
const testingDevices = ref(false)
const testingChannels = ref(false)
const uplinkTemplate = ref<UplinkConfig | null>(null)
/** 异常码参考区默认收起，避免一屏塞进整张码表；点标题展开 */
const errorPanels = ref<string[]>([])

const hasItems = computed(() => devices.value.length > 0 || channels.value.length > 0)

const summary = computed<TestSummary>(() => {
  const all = [...devices.value, ...channels.value]
  const tested = all.filter((r) => r._status === 'ok' || r._status === 'fail')
  const pass = all.filter((r) => r._status === 'ok').length
  const fail = all.filter((r) => r._status === 'fail').length
  const latencies = tested.map((r) => r._latency ?? 0).filter((n) => n > 0)
  const avg = latencies.length ? Math.round(latencies.reduce((a, b) => a + b, 0) / latencies.length) : 0
  const lastAt = tested.length > 0 ? formatClock() : ''
  return { total: tested.length, pass, fail, avgLatency: avg, lastAt }
})

/** 加载设备列表 + 上行通道状态 */
async function loadAll() {
  loading.value = true
  try {
    const [devs, north] = await Promise.all([
      listDevices(),
      getNorthStatus().catch(
        (): NorthStatus => ({
          healthy: false,
          channels: [],
          aggregated: {
            mqtt: 0,
            http: 0,
            mqtt_connected: 0,
            http_connected: 0,
            pending_messages: 0,
            total_messages_sent: 0,
          },
          servers: [],
        }),
      ),
    ])
    devices.value = (devs || []).map((d) => ({ ...d, _status: 'idle' }))
    channels.value = (north.channels || []).map((c) => ({ ...c, _status: 'idle' }))
  } finally {
    loading.value = false
  }
}

/** 测试单个设备：取第一个点位读取 */
async function testDevice(dev: DeviceRow) {
  dev._status = 'testing'
  dev._error = undefined
  dev._latency = undefined
  try {
    const pts = await listPoints(dev.id)
    if (pts.length === 0) {
      dev._status = 'fail'
      dev._error = '无点位，无法读取'
      return
    }
    const t0 = performance.now()
    await readPointNow(pts[0]!.id)
    dev._latency = Math.round(performance.now() - t0)
    dev._status = 'ok'
  } catch (e: unknown) {
    dev._status = 'fail'
    dev._error = extractErrorMessage(e)
  }
}

/** 批量测试并发度：每批 5 个设备并发执行，避免设备过多时串行阻塞 UI */
const TEST_CONCURRENCY = 5

/** 测试所有设备（分批并发，每批 TEST_CONCURRENCY 个，失败不中断后续批次） */
async function testAllDevices() {
  testingDevices.value = true
  try {
    const list = devices.value
    for (let i = 0; i < list.length; i += TEST_CONCURRENCY) {
      const batch = list.slice(i, i + TEST_CONCURRENCY)
      // allSettled：单个设备失败不影响同批其他设备，错误已在 testDevice 内捕获并写入 _error
      await Promise.allSettled(batch.map((d) => testDevice(d)))
    }
  } finally {
    testingDevices.value = false
  }
  ElMessage.success(`设备测试完成：${devices.value.filter((d) => d._status === 'ok').length}/${devices.value.length} 通过`)
}

/** 测试单个上行通道 */
async function testChannel(ch: ChannelRow) {
  ch._status = 'testing'
  ch._error = undefined
  ch._latency = undefined
  ch._detail = undefined
  try {
    // 首次测试时懒加载上行模板
    if (!uplinkTemplate.value) {
      uplinkTemplate.value = await getUplinkConfig()
    }
    const tpl = uplinkTemplate.value?.template || '{"device":"${device_id}","value":${value}}'
    const t0 = performance.now()
    const resp = await testUplink({
      template: tpl,
      vars: { device_id: 'test-device', point_id: 'test-point', value: 1, quality_code: 0 },
      send: true,
    })
    ch._latency = Math.round(performance.now() - t0)
    if (resp.ok) {
      ch._status = 'ok'
      ch._detail = resp.publish
        ? `topic=${resp.publish.topic}, qos=${resp.publish.qos}, ${resp.publish.delivered ? '已送达' : '未确认送达'}`
        : '渲染成功（未发送）'
    } else {
      ch._status = 'fail'
      ch._error = resp.error || '测试失败'
    }
  } catch (e: unknown) {
    ch._status = 'fail'
    ch._error = extractErrorMessage(e)
  }
}

/** 测试所有上行通道 */
async function testAllChannels() {
  testingChannels.value = true
  for (const c of channels.value) {
    await testChannel(c)
  }
  testingChannels.value = false
  ElMessage.success(`通道测试完成：${channels.value.filter((c) => c._status === 'ok').length}/${channels.value.length} 通过`)
}

/** 一键测试全部 */
async function testAll() {
  testing.value = true
  await Promise.all([testAllDevices(), testAllChannels()])
  testing.value = false
  const pass = summary.value.pass
  const fail = summary.value.fail
  ElMessage.success(`全部测试完成：通过 ${pass}，失败 ${fail}`)
}

onMounted(loadAll)
</script>

<style scoped src="./ConnectionTest.css"></style>