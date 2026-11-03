<template>
  <div class="config">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="运行参数" icon="Setting">
          <!-- 按钮分两簇：只读操作（读/导出/校验/预览）在前，写入操作（应用/保存）在后，
               中间用分隔线隔开，避免「预览变更」与「热重载」看起来像同一类动作 -->
          <el-button size="small" @click="loadConfig">
            <el-icon><Refresh /></el-icon> 重新读取
          </el-button>
          <el-button type="success" size="small" @click="exportConfig">
            <el-icon><Download /></el-icon> 导出
          </el-button>
          <!-- P2b: 现场配置耗时长，提供本地草稿，避免断电/误关标签页丢失输入 -->
          <el-tooltip
            content="把当前未保存的改动存到浏览器本地（不写入网关），下次进入本页可恢复"
            placement="bottom"
          >
            <el-button v-if="canWriteConfig" size="small" @click="saveDraft">
              <el-icon><Files /></el-icon> 存草稿
            </el-button>
          </el-tooltip>
          <el-button size="small" @click="showValidate = true">
            <el-icon><CircleCheck /></el-icon> 校验配置
          </el-button>
          <el-button size="small" @click="previewDiff">
            <el-icon><Sort /></el-icon> 预览变更
          </el-button>

          <el-divider direction="vertical" />

          <el-tooltip
            content="按磁盘上的配置文件重新初始化采集，页面上的未保存改动会被丢弃"
            placement="bottom"
          >
            <el-button v-if="canReloadConfig" type="info" size="small" :loading="reloading" @click="doReload">
              <el-icon><RefreshRight /></el-icon> 仅应用不保存
            </el-button>
          </el-tooltip>
          <!-- 修复(审查#B7)：保存按钮绑定在飞保护，避免双击叠加两次确认与热重载 -->
          <el-button
            v-if="canWriteConfig"
            type="primary"
            size="small"
            :loading="saving"
            :disabled="saving"
            @click="saveConfig"
          >
            <el-icon><Check /></el-icon> 保存并应用
          </el-button>
        </SectionHeader>
      </template>

      <ConfigValidateDialog v-model="showValidate" :config="gatewayConfig" :parse-error="parseError" @locate="onLocateIssue" @save="saveConfig" />

      <!-- P2: 配额上限此前只在顶栏授权弹窗里可见，用户在配置页加点位时毫不知情，
           直到保存被 4021 拒绝才知道超限。这里常驻展示「档位 + 已用/上限」，达上限时变色预警。 -->
      <div class="quota-bar" :class="{ 'quota-bar--limit': quotaAtLimit }">
        <el-icon><InfoFilled /></el-icon>
        <span>当前档位<b>{{ tierLabel }}</b></span>
        <template v-if="maxPoints !== null">
          <span class="sep">·</span>
          <span>
            采集点数已用 <b :class="{ 'quota-over': quotaAtLimit }">{{ usedPoints ?? '—' }}</b>
            / 上限 {{ maxPoints }}
          </span>
          <span v-if="quotaAtLimit" class="quota-warn">已达上限，新增点位保存时会被拒绝</span>
        </template>
        <span v-else class="sep">· 采集点数不限</span>
      </div>

      <!-- P2b: 上次会话存下的本地草稿。默认不自动套用（草稿可能是过期的旧配置），
           由用户显式选择「恢复」或「丢弃」。 -->
      <el-alert
        v-if="hasDraft"
        class="draft-alert"
        type="info"
        :closable="false"
        show-icon
      >
        <template #title>
          检测到本地草稿（保存于 {{ draftTimeText }}），与网关当前配置不同
        </template>
        <div class="draft-actions">
          <span class="draft-hint">恢复只会填入表单，仍需「保存并应用」才会生效</span>
          <el-button size="small" type="primary" @click="restoreDraft">恢复草稿</el-button>
          <el-button size="small" plain @click="discardDraft">丢弃</el-button>
        </div>
      </el-alert>

      <el-tabs v-model="activeTab" class="config-tabs">
        <el-tab-pane name="buses">
          <template #label>
            <span class="tab-label"><el-icon><Connection /></el-icon> 总线</span>
          </template>
          <BusConfigTab
            v-model="config.buses"
            :port-errors="busPortErrors"
            @add="addBus"
            @remove="removeBus"
            @protocol-change="onBusProtocolChange"
          />
        </el-tab-pane>

        <el-tab-pane name="web">
          <template #label>
            <span class="tab-label"><el-icon><Monitor /></el-icon> Web</span>
          </template>
          <WebConfigTab
            v-model="config.web"
            :original-token="originalToken"
            :token-status-text="tokenStatusText"
          />
        </el-tab-pane>

        <el-tab-pane name="mqtt">
          <template #label>
            <span class="tab-label"><el-icon><ChatDotRound /></el-icon> MQTT</span>
          </template>
          <MqttConfigTab v-model="config.mqtt" />
        </el-tab-pane>

        <el-tab-pane name="ntp">
          <template #label>
            <span class="tab-label"><el-icon><Clock /></el-icon> NTP</span>
          </template>
          <NtpConfigTab v-model="config.ntp" />
        </el-tab-pane>

        <el-tab-pane name="telemetry">
          <template #label>
            <span class="tab-label"><el-icon><DataLine /></el-icon> 遥测</span>
          </template>
          <TelemetryConfigTab v-model="config.telemetry" />
        </el-tab-pane>

        <el-tab-pane name="webhook">
          <template #label>
            <span class="tab-label"><el-icon><Promotion /></el-icon> Webhook</span>
          </template>
          <WebhookConfigTab v-model="config.webhook" />
        </el-tab-pane>

        <el-tab-pane name="opcua">
          <template #label>
            <span class="tab-label"><el-icon><Share /></el-icon> OPC UA Server</span>
          </template>
          <OpcuaConfigTab v-model="config.opcua" />
        </el-tab-pane>

        <el-tab-pane name="modbus-server">
          <template #label>
            <span class="tab-label"><el-icon><Cpu /></el-icon> Modbus Server</span>
          </template>
          <ModbusServerConfigTab v-model="config.modbus_server" />
        </el-tab-pane>

        <el-tab-pane name="modbus-passthrough">
          <template #label>
            <span class="tab-label"><el-icon><Link /></el-icon> Modbus 透传</span>
          </template>
          <ModbusPassthroughConfigTab v-model="config.modbus_passthrough" :buses="config.buses" />
        </el-tab-pane>

        <el-tab-pane name="raw">
          <template #label>
            <span class="tab-label" :class="{ 'has-error': parseError }">
              <el-icon><Document /></el-icon> 原始 JSON
              <el-icon v-if="parseError" class="tab-error-icon"><WarningFilled /></el-icon>
            </span>
          </template>
          <RawJsonTab
            v-model="rawText"
            :parse-error="parseError"
            :parse-error-message="parseErrorMessage"
            :raw-json-html="rawJsonHtml"
            @reset="resetRawText"
            @input="onRawJsonInput"
            @scroll="syncScroll"
          />
        </el-tab-pane>
      </el-tabs>
    </el-card>

    <ConfigChangePreview v-model="showDiff" :config="gatewayConfig" :parse-error="parseError" @save="saveConfig" />

    <!-- B5：参数同步并入运行参数。要同步的超时/重试就是本页「总线」节正在编辑的字段，
         原独立页签让「先看当前值、再回上页改」多一次跳转；这里收成默认收起的折叠区块，
         展开后才请求配置，避免仅查看参数时多打一次接口 -->
    <el-card shadow="never" class="section-card">
      <el-collapse v-model="paramSyncPanels">
        <el-collapse-item name="param-sync" title="参数同步（同网关总线间同步）">
          <ParamSync v-if="paramSyncPanels.includes('param-sync')" embedded />
        </el-collapse-item>
      </el-collapse>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useConfigActions } from '@/composables/config/useConfigActions'
import { usePermission } from '@/composables/auth/usePermission'
import { useTierStore, TIER_LABEL } from '@/stores/tier'
import { getStatus } from '@/api/status'
import ConfigChangePreview from './dialogs/ConfigChangePreview.vue'
import ConfigValidateDialog from './dialogs/ConfigValidateDialog.vue'
import BusConfigTab from './tabs/BusConfigTab.vue'
import WebConfigTab from './tabs/WebConfigTab.vue'
import MqttConfigTab from './tabs/MqttConfigTab.vue'
import NtpConfigTab from './tabs/NtpConfigTab.vue'
import TelemetryConfigTab from './tabs/TelemetryConfigTab.vue'
import WebhookConfigTab from './tabs/WebhookConfigTab.vue'
import OpcuaConfigTab from './tabs/OpcuaConfigTab.vue'
import ModbusServerConfigTab from './tabs/ModbusServerConfigTab.vue'
import ModbusPassthroughConfigTab from './tabs/ModbusPassthroughConfigTab.vue'
import RawJsonTab from './tabs/RawJsonTab.vue'
import ParamSync from './ParamSync.vue'
import { protocolDefaultTimeoutMs, isProtocolDefaultTimeout } from '@/composables/devices/useProtocol'
import type { BusConfig } from '@/api/types'

const {
  activeTab, showValidate, showDiff,
  reloading, saving, config, gatewayConfig, originalToken, rawText, parseError, parseErrorMessage,
  tokenStatusText, rawJsonHtml, hasDraft, draftSavedAt,
  loadConfig, saveConfig, doReload, exportConfig, previewDiff,
  addBus, removeBus, saveDraft, restoreDraft, discardDraft,
  resetRawText, onRawJsonInput, syncScroll, onLocateIssue,
} = useConfigActions()

// 按钮级权限：保存/模板需 config.write，热重载需 config.reload
const { can } = usePermission()
const canWriteConfig = computed(() => can('config.write'))
const canReloadConfig = computed(() => can('config.reload'))

/** B5：页内「参数同步」折叠区当前展开项（默认收起） */
const paramSyncPanels = ref<string[]>([])

// P2: 配额常驻提示。上限来自 license 配额（null = 不限），已用点数取运行态 total_points
// （与后端 count_points 的展开口径一致，不是 config.points 的显式条数）。
const tierStore = useTierStore()
/** 无授权文件时不能显示「核心版」冒充已授权，否则用户会以为自己有配额 */
const tierLabel = computed(() => {
  const s = tierStore.licenseState
  if (s && !s.present) return '未授权'
  return TIER_LABEL[tierStore.current] ?? tierStore.current
})
const maxPoints = computed<number | null>(() => {
  const q = tierStore.licenseState?.quota?.max_points
  return typeof q === 'number' ? q : null
})
const usedPoints = ref<number | null>(null)
const quotaAtLimit = computed(
  () => usedPoints.value !== null && maxPoints.value !== null && usedPoints.value >= maxPoints.value,
)

async function refreshQuotaUsage() {
  try {
    const s = await getStatus()
    const n = Number(s.total_points)
    usedPoints.value = Number.isFinite(n) ? n : null
  } catch {
    /* 状态接口不可用时静默：配额提示退化为只显示上限 */
  }
}

onMounted(async () => {
  await refreshQuotaUsage()
  // license 快照若尚未拉取（例如直接进入本页），补拉一次以获得上限
  if (!tierStore.licenseState) await tierStore.refreshFromLicense()
})

// 保存后点位可能增减，刷新一次「已用」避免提示停留在旧值
watch(saving, (now, before) => {
  if (before && !now) void refreshQuotaUsage()
})

// 草稿保存时间的可读展示（本地草稿提示用）
const draftTimeText = computed(() => {
  if (!draftSavedAt.value) return '未知时间'
  const d = new Date(draftSavedAt.value)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getMonth() + 1}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
})

// A1: 总线 tab 实时检测 port_name 重复；第二次起的行显示行内错误。
// trim 后判等与 checkBusConfig / addBus 保持一致契约。
const busPortErrors = computed(() => {
  const errs = new Map<number, string>()
  const seen = new Map<string, number>()
  config.buses.forEach((b, i) => {
    const key = b.port_name?.trim()
    if (!key) return
    const first = seen.get(key)
    if (first !== undefined) {
      errs.set(i, `端口 "${key}" 与第 ${first + 1} 条重复`)
    } else {
      seen.set(key, i)
    }
  })
  return errs
})

// 切换总线协议时同步默认超时：仅当超时仍是「协议默认值」（用户未在通信参数页自定义过）
// 才跟随更新为新协议的默认值，避免覆盖用户显式设置的超时。
//
// 同时清空变体与对端地址：两者都带协议语义（如 dlt645 的 1997 子变体、S7 的 ?rack/slot），
// 残留上一个协议的取值会让新协议按错误参数解析，现场表现为「配好了但采不到数」。
function onBusProtocolChange(bus: BusConfig, next: string) {
  bus.protocol = next
  bus.variant = null
  bus.tcp_addr = null
  // OPC UA 认证块只对 opcua 生效：切走或切回都先归零，避免旧凭据残留到新协议/新端点。
  bus.opcua = null
  if (isProtocolDefaultTimeout(bus.timeout_ms)) {
    bus.timeout_ms = protocolDefaultTimeoutMs(next)
  }
}
</script>

<style scoped src="./Config.css"></style>