<template>
  <div class="fwd">
    <SectionHeader title="通道转发" icon="Promotion" />

    <div class="top-bar">
      <el-button type="primary" @click="goRuntimeConfig(primarySection)">
        <el-icon><Setting /></el-icon> 去运行参数配置
      </el-button>
      <el-button @click="load"><el-icon><Refresh /></el-icon> 刷新</el-button>
      <span class="top-hint">本页只做启用开关与连通性测试；Broker / Topic / 账号等字段统一在「系统 › 运行参数」编辑</span>
    </div>

    <!-- 空表不能只留一句「暂无转发通道」：配置里没有 mqtt/webhook 段时，
         用户不知道去哪开。这里直接给出下一步动作。 -->
    <el-alert
      v-if="!loading && channels.length === 0"
      type="info"
      :closable="false"
      show-icon
      class="empty-guide"
      title="当前配置里没有 MQTT / HTTP 上报段，因此没有可管理的转发通道"
    >
      <p class="empty-guide-hint">
        点下方「去运行参数配置」填写 Broker 等连接参数并保存后，通道会自动出现在这里。
      </p>
      <!-- 直接落到具体的配置小节，而不是运行参数总页让用户再自己找 -->
      <div class="guide-actions">
        <el-button size="small" type="primary" @click="goRuntimeConfig('mqtt')">去配置 MQTT</el-button>
        <el-button size="small" plain @click="goRuntimeConfig('webhook')">配置 Webhook</el-button>
      </div>
    </el-alert>

    <!-- 通道存在但全部停用：表格里有内容，用户却看不到「为什么没数据」，需要显式提示 -->
    <el-alert
      v-else-if="!loading && channels.length > 0 && !channels.some((c) => c.enabled)"
      type="warning"
      :closable="false"
      show-icon
      class="empty-guide"
      title="所有转发通道当前都处于停用状态，网关不会向外上报数据"
    >
      <p class="empty-guide-hint">
        打开表格里的「启用」开关即可上报，无需离开本页；MQTT 开关即时生效，HTTP 转发需重启网关后生效；要改 Broker / Topic 请点「参数」到运行参数。
      </p>
      <div class="guide-actions">
        <el-button size="small" type="primary" @click="goRuntimeConfig(primarySection)">前往运行参数</el-button>
      </div>
    </el-alert>

    <DataTable
      :data="channels"
      :columns="channelColumns"
      row-key="id"
      :loading="loading"
      stripe
      empty-text="暂无转发通道"
    />

    <el-dialog v-model="showTest" title="连接测试" width="480px">
      <el-result :icon="testResult.ok ? 'success' : 'error'" :title="testResult.ok ? '连接成功' : '连接失败'">
        <template #sub-title>
          <div v-if="testResult.latency !== null">延迟：{{ testResult.latency }} ms</div>
          <div>{{ testResult.message }}</div>
        </template>
      </el-result>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, h } from 'vue'
import { useRouter } from 'vue-router'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElButton, ElMessage, ElSwitch, ElTag } from 'element-plus'
import { Refresh, Setting } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import { getConfig, postConfig } from '@/api/config'
import { getUplinkConfig, getNorthStatus, testUplink } from '@/api/uplink'
import { testWebhook } from '@/api/webhook'

type ForwardType = 'mqtt' | 'http'

interface Channel {
  id: string
  name: string
  type: ForwardType
  enabled: boolean
  broker: string
  port: number
  username: string
  password: string
  topic: string
  qos: number
  reportInterval: number
  url: string
  headers: Record<string, string | undefined>
  /**
   * 运行态连接情况（来自 GET /api/north/status）。
   * 配置里的 enabled 只说明「打算上报」，不等于「真的连上了 broker」——
   * broker 不可达时配置仍是启用，但网关不会上报任何数据。
   * 不显示这一列，用户会以为「已启用 = 数据在发」。
   */
  connected: boolean | null
}

const router = useRouter()
const channels = ref<Channel[]>([])
const loading = ref(false)
const showTest = ref(false)
/** 测试中：连接测试往返期间禁用该行的测试按钮，避免并发重复探测 */
const testing = ref(false)
/** 测试结果：latency 为 null 表示后端未提供延迟数据（不应显示 0 ms） */
const testResult = ref({ ok: false, latency: null as number | null, message: '' })

function typeTag(t: ForwardType) { return t === 'mqtt' ? 'primary' : 'success' }

// 转发通道表统一走 DataTable：列顺序/宽度与迁移前保持一致
const channelColumns: ColumnDef<Channel>[] = [
  { key: 'name', title: '通道名称', width: 140 },
  {
    key: 'type',
    title: '协议',
    width: 90,
    cell: (row) => h(ElTag, { size: 'small', type: typeTag(row.type) }, () => row.type.toUpperCase()),
  },
  {
    key: 'target',
    title: '目标',
    minWidth: 180,
    ellipsis: true,
    cell: (row) => {
      if (row.type === 'mqtt') return `${row.broker} / ${row.topic}`
      return row.url
    },
  },
  {
    key: 'reportInterval',
    title: '上报周期',
    width: 100,
    cell: (row) => `${row.reportInterval}s`,
  },
  {
    key: 'enabledState',
    title: '状态',
    width: 130,
    cell: (row) => {
      if (!row.enabled) {
        return h(ElTag, { size: 'small', type: 'info', effect: 'dark', round: true }, () => '已停用')
      }
      // 已启用但未连上：这是最容易误判的状态（以为在发数据，其实一条都没发出去）
      if (row.connected === false) {
        return h(ElTag, { size: 'small', type: 'danger', effect: 'dark', round: true }, () => '已启用·未连接')
      }
      if (row.connected === true) {
        return h(ElTag, { size: 'small', type: 'success', effect: 'dark', round: true }, () => '已启用·已连接')
      }
      return h(ElTag, { size: 'small', type: 'warning', effect: 'dark', round: true }, () => '已启用·状态未知')
    },
  },
  {
    key: 'enabled',
    title: '启用',
    width: 80,
    cell: (row) =>
      h(ElSwitch, {
        modelValue: row.enabled,
        'onUpdate:modelValue': (v: string | number | boolean) => { row.enabled = Boolean(v) },
        onChange: () => onToggle(row),
      }),
  },
  actionColumn<Channel>({
    width: 160,
    fixed: 'right',
    cell: (row) => [
      h(ElButton, { size: 'small', type: 'success', link: true, disabled: testing.value, onClick: () => testConn(row) }, () => '测试'),
      // A2：字段编辑统一回「运行参数」，本页不再提供第二份编辑表单
      h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => goRuntimeConfig(sectionOf(row.type)) }, () => '参数'),
    ],
  }),
]

/** 通道类型 → 运行参数页的分节名（HTTP 段在配置里叫 webhook） */
function sectionOf(t: ForwardType): 'mqtt' | 'webhook' {
  return t === 'http' ? 'webhook' : 'mqtt'
}

/** 默认落点：优先 MQTT（最常用），否则取第一条通道，再退化为 mqtt */
const primarySection = computed<'mqtt' | 'webhook'>(() => {
  const first = channels.value.find((c) => c.type === 'mqtt') ?? channels.value[0]
  return first ? sectionOf(first.type) : 'mqtt'
})

/** 跳到运行参数页的 MQTT/Webhook 分节 */
function goRuntimeConfig(section: 'mqtt' | 'webhook') {
  void router.push({ path: '/system/config', query: { tab: 'runtime-config', section } })
}

async function load() {
  loading.value = true
  try {
    const cfg = await getConfig()
    // 运行态连接情况：配置启用 ≠ 已连上。取不到就退化成 null（未知），不谎报。
    let connected: Record<string, boolean | null> = {}
    try {
      const ns = await getNorthStatus()
      for (const ch of ns.channels ?? []) connected[ch.type] = ch.connected
    } catch { /* 运行态不可用时按未知处理 */ }
    const list: Channel[] = []
    if (cfg.mqtt) {
      list.push({
        id: 'mqtt', name: 'MQTT 转发', type: 'mqtt', enabled: cfg.mqtt.enabled,
        broker: cfg.mqtt.broker, port: 1883, username: cfg.mqtt.username || '', password: '',
        topic: cfg.mqtt.topic, qos: cfg.mqtt.qos, reportInterval: cfg.mqtt.publish_interval,
        url: '', headers: {}, connected: connected['mqtt'] ?? null,
      })
    }
    if (cfg.webhook) {
      list.push({
        id: 'http', name: 'HTTP Webhook', type: 'http', enabled: cfg.webhook.enabled,
        broker: '', port: 0, username: '', password: '', topic: '', qos: 0, reportInterval: 0,
        url: cfg.webhook.url, headers: cfg.webhook.headers || {},
        connected: connected['http'] ?? null,
      })
    }
    channels.value = list
  } finally { loading.value = false }
}

async function onToggle(c: Channel) {
  try {
    const cfg = await getConfig()
    if (c.type === 'mqtt' && cfg.mqtt) cfg.mqtt.enabled = c.enabled
    else if (c.type === 'http' && cfg.webhook) cfg.webhook.enabled = c.enabled
    await postConfig(cfg)
    // HTTP 转发器只在启动时创建，开关改动落盘但不热更，必须明确告知需要重启
    ElMessage.success(
      c.type === 'http'
        ? `${c.enabled ? '已启用' : '已停用'}（需重启网关后生效）`
        : c.enabled ? '已启用' : '已停用',
    )
  } catch (e) { showToast(e, '操作失败') }
}

/**
 * MQTT 测试失败原因翻译。
 *
 * 后端在「北向没有 MQTT 客户端实例」时统一返回
 * 「MQTT 客户端未启动（mqtt.enabled=false）」。但网关启动时 broker 连不上，
 * 客户端同样会缺席，而配置里 mqtt.enabled 仍是 true —— 直接把这句话透给用户，
 * 会让人跑去把「启用」开关又关又开，方向完全错。
 *
 * 这里按「配置是否启用」区分两种成因，给出各自该做的下一步。
 */
function explainMqttError(err: string | undefined, c: Channel): string {
  const raw = err || '上行测试失败：MQTT 客户端未启动或模板编译失败'
  if (!raw.includes('MQTT 客户端未启动')) return raw
  if (!c.enabled) {
    return 'MQTT 通道当前是停用状态，网关不会上报任何数据。请先在上表打开「启用」开关并保存。'
  }
  return `MQTT 已启用，但网关未能与 ${c.broker || '配置的 Broker'} 建立连接（配置正确、链路不通）。请检查 Broker 地址、端口与网络连通性，必要时到「系统 › 日志」查看连接错误。`
}

/**
 * 通道连接测试：真实发起一次连通性探测，不再「只看字段非空就报成功」。
 *
 * 此前实现是纯前端模拟：sleep 300ms 后判断 `!!c.broker`，只要地址非空就报「已连接」——
 * broker 根本不可达时用户同样看到「连接成功」，现场排查方向被直接带偏。
 *
 * 现在按协议走真实探测：
 * - MQTT：复用 POST /api/uplink/test（send=true），由网关向真实 broker 发一条并回执
 *   delivered / latency_ms；
 * - HTTP：复用 POST /api/webhook/test，对配置里的 URL 发一次真实请求。
 */
async function testConn(c: Channel) {
  if (testing.value) return
  testing.value = true
  const started = Date.now()
  try {
    if (c.type === 'mqtt') {
      const uplink = await getUplinkConfig()
      const r = await testUplink({
        template: uplink.template || '{"point_id":"${point_id}","value":${value}}',
        vars: { point_id: 'connection_test', device_id: 'connection_test', value: 1 },
        send: true,
      })
      const latency = r.publish?.latency_ms ?? Math.round(Date.now() - started)
      testResult.value = {
        ok: !!r.ok && !!r.publish?.delivered,
        latency,
        message: r.ok
          ? (r.publish?.delivered
              ? `已向 ${c.broker || '配置的 Broker'} 成功发布一条测试消息（Topic：${r.publish?.topic || c.topic || '-'}）`
              : r.error || '消息未成功送达 Broker')
          : explainMqttError(r.error, c),
      }
    } else if (c.type === 'http') {
      // 配置里的 headers 允许缺值（undefined），探测时统一按空串处理
      const headers: Record<string, string> = {}
      for (const [k, v] of Object.entries(c.headers || {})) headers[k] = v ?? ''
      const r = await testWebhook({
        url: c.url,
        method: 'POST',
        headers,
        body: JSON.stringify({ test: true, ts: Date.now() }),
      })
      testResult.value = {
        ok: !!r.ok,
        latency: typeof r.latency === 'number' ? r.latency : null,
        message: r.ok
          ? `${c.url} 返回 HTTP ${r.status} ${r.status_text}`
          : r.error || `请求失败（HTTP ${r.status} ${r.status_text}）`,
      }
    }
  } catch (e) {
    testResult.value = {
      ok: false,
      latency: null,
      message: e instanceof Error && e.message ? e.message : '连接测试请求失败',
    }
  } finally {
    testing.value = false
    showTest.value = true
  }
}

onMounted(load)
</script>

<style scoped>
.fwd { padding: 0px; }
.top-bar { display: flex; gap: 8px; align-items: center; margin-bottom: 12px; }
/* 顶部说明：解释「为什么本页没有编辑表单」，避免用户以为功能缺失 */
.top-hint { font-size: 0.8125rem; color: var(--text-secondary); }
.empty-guide { margin-bottom: 12px; }
.empty-guide-hint { margin: 0 0 8px; font-size: 0.8125rem; color: var(--text-secondary); }
.guide-actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
</style>