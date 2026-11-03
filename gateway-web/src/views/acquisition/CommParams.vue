<template>
  <div class="cp">
    <SectionHeader title="通信参数" icon="Setting" />

    <!-- A1：超时/重试是总线级字段，必须按总线成行展示。
         原实现一设备一行却展示总线值，编辑任意一行都会改动整条总线，
         同总线多设备之间看起来「可以各配一套」，实际是同一份值。 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <strong>总线级通信参数</strong>
        <span class="hint">超时 / 重试作用于整条总线，保存后该总线上的所有设备一起生效</span>
      </template>
      <DataTable
        :data="buses"
        :columns="busColumns"
        row-key="portName"
        stripe
        :loading="loading"
        empty-text="暂无总线，请先在「配置管理 › 运行参数 › 总线」中添加"
      />
    </el-card>

    <el-card shadow="never" class="section-card">
      <template #header>
        <strong>设备级字节序</strong>
        <span class="hint">字节序按设备生效，保存后写入该设备的全部点位</span>
      </template>
      <DataTable
        :data="devices"
        :columns="devColumns"
        row-key="deviceId"
        stripe
        :loading="loading"
        empty-text="暂无设备，请先在「设备管理 › 设备清单」中添加设备"
      />
    </el-card>

    <el-alert title="说明：通信失败超过重试次数后，设备将进入降级状态。字节序修改会应用到该设备的所有点位。" type="info" :closable="false" show-icon />
  </div>
</template>

<script setup lang="ts">
import { h, ref, onMounted } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElInputNumber, ElMessage, ElNotification, ElOption, ElSelect } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { getConfig, postConfig } from '@/api/config'
import { batchEditPoints } from '@/api/points'
import type { DeviceConfig, BusConfig, PointConfig } from '@/api/types'

const BYTE_ORDERS = ['ABCD', 'DCBA', 'CDAB', 'BADC']

/** 总线行：一行对应一条总线（而非一台设备），行内字段与 cfg.buses 字段一一对应 */
interface BusRow {
  portName: string
  deviceCount: number
  timeoutMs: number
  /** 后端 retry_count 为 Option<u32>，生成的 TS 类型是 number | null */
  retryCount: number | null
  retryDelayMs: number
}

interface DevRow {
  deviceId: string
  deviceName: string
  busName: string
  byteOrder: string
}

const buses = ref<BusRow[]>([])
const devices = ref<DevRow[]>([])
const loading = ref(false)

// 总线级列：每行一条总线，一行对应 cfg.buses 的一条记录
const busColumns: ColumnDef<BusRow>[] = [
  { key: 'portName', title: '总线', minWidth: 160 },
  {
    key: 'deviceCount',
    title: '设备数',
    width: 110,
    cell: (row) => h('span', null, `${row.deviceCount} 台`),
  },
  {
    key: 'timeoutMs',
    title: '超时(ms)',
    width: 150,
    cell: (row) =>
      h(ElInputNumber, {
        modelValue: row.timeoutMs,
        min: 100,
        step: 100,
        size: 'small',
        controlsPosition: 'right',
        'onUpdate:modelValue': (v: unknown) => { row.timeoutMs = Number(v) },
        onChange: () => saveBus(row),
      }),
  },
  {
    key: 'retryCount',
    title: '重试次数',
    width: 130,
    cell: (row) =>
      h(ElInputNumber, {
        modelValue: row.retryCount,
        min: 0,
        max: 10,
        size: 'small',
        controlsPosition: 'right',
        'onUpdate:modelValue': (v: unknown) => { row.retryCount = Number(v) },
        onChange: () => saveBus(row),
      }),
  },
  {
    key: 'retryDelayMs',
    title: '重试间隔(ms)',
    width: 160,
    cell: (row) =>
      h(ElInputNumber, {
        modelValue: row.retryDelayMs,
        min: 50,
        step: 50,
        size: 'small',
        controlsPosition: 'right',
        'onUpdate:modelValue': (v: unknown) => { row.retryDelayMs = Number(v) },
        onChange: () => saveBus(row),
      }),
  },
]

const devColumns: ColumnDef<DevRow>[] = [
  { key: 'deviceName', title: '设备', minWidth: 150 },
  { key: 'busName', title: '所属总线', width: 140 },
  {
    key: 'byteOrder',
    title: '字节序',
    width: 160,
    cell: (row) =>
      h(
        ElSelect,
        {
          modelValue: row.byteOrder,
          size: 'small',
          'onUpdate:modelValue': (v: unknown) => { row.byteOrder = String(v) },
          onChange: () => saveByteOrder(row),
        },
        { default: () => BYTE_ORDERS.map((b) => h(ElOption, { key: b, label: b, value: b })) },
      ),
  },
]

async function load() {
  loading.value = true
  try {
    const cfg = await getConfig()
    const busList = cfg.buses || []
    const devList = cfg.devices || []
    const points = cfg.points || []

    const devCountByBus = new Map<string, number>()
    for (const d of devList) {
      devCountByBus.set(d.bus, (devCountByBus.get(d.bus) ?? 0) + 1)
    }

    buses.value = busList.map((b: BusConfig) => ({
      portName: b.port_name,
      deviceCount: devCountByBus.get(b.port_name) ?? 0,
      timeoutMs: b.timeout_ms,
      retryCount: b.retry_count,
      retryDelayMs: b.retry_delay_ms,
    }))

    // 字节序取该设备首个点位的值，仅用于展示；保存时整设备统一写入
    const pointMap = new Map<string, PointConfig>()
    for (const p of points) {
      if (p.device_id && !pointMap.has(p.device_id)) pointMap.set(p.device_id, p)
    }
    devices.value = devList.map((d: DeviceConfig) => ({
      deviceId: d.id,
      deviceName: d.name,
      busName: d.bus,
      byteOrder: pointMap.get(d.id)?.byte_order || 'ABCD',
    }))
  } catch (e) {
    showToast(e, '加载通信参数失败')
  } finally {
    loading.value = false
  }
}

/** 保存总线参数：行即总线，写入口径与展示口径一致 */
async function saveBus(row: BusRow) {
  try {
    const cfg = await getConfig()
    const bus = (cfg.buses || []).find((b: BusConfig) => b.port_name === row.portName)
    if (!bus) {
      ElMessage.warning(`总线 ${row.portName} 已不存在，请刷新后重试`)
      return
    }
    bus.timeout_ms = row.timeoutMs
    bus.retry_count = row.retryCount
    bus.retry_delay_ms = row.retryDelayMs
    await postConfig(cfg)
    ElMessage.success(`已保存总线 ${row.portName} 的参数（影响 ${row.deviceCount} 台设备）`)
  } catch (e) {
    showToast(e, '保存失败')
  }
}

async function saveByteOrder(row: DevRow) {
  try {
    const cfg = await getConfig()
    const devPoints = (cfg.points || []).filter((p: PointConfig) => p.device_id === row.deviceId)
    if (!devPoints.length) {
      ElMessage.warning('该设备无点位')
      return
    }
    const updated = devPoints.map((p: PointConfig) => ({ ...p, byte_order: row.byteOrder }))
    await batchEditPoints({ add: [], update: updated, delete: [] })
    ElMessage.success(`已更新 ${row.deviceName} 的字节序`)
  } catch (e) {
    showToast(e, '保存失败')
    // 整设备走单次批量接口（1200+ 点位不可拆），响应无逐条结果：
    // 失败不等于全部未生效，需告知用户逐条核对，而不是让用户自行猜测
    ElNotification({
      type: 'warning',
      title: '字节序保存结果未知',
      duration: 0,
      message: '批量接口未返回逐条结果，若部分点位未生效请重新应用或逐条核对。',
    })
  }
}

onMounted(load)
</script>

<style scoped>
.cp { padding: 16px; }
.section-card { margin-bottom: 16px; }
.hint { margin-left: 8px; font-size: 0.75rem; color: var(--el-text-color-secondary); }
</style>
