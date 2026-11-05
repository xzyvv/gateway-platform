<template>
  <!-- 授权管理弹窗：设备码复制 → 供应商签发 → 导入 .lic 全流程 -->
  <el-dialog
    :model-value="modelValue"
    title="授权管理"
    width="720px"
    :close-on-click-modal="false"
    @update:model-value="(v: boolean) => $emit('update:modelValue', v)"
    @open="onOpen"
  >
    <div
      v-loading="loading"
      element-loading-text="加载中..."
      element-loading-background="rgba(255, 255, 255, 0.9)"
    >
      <!-- 当前授权状态 -->
      <h4 class="lic-section-title">当前授权</h4>
      <el-descriptions v-if="snapshot" :column="1" size="small" border>
        <el-descriptions-item label="档位">{{ TIER_LABEL[snapshot.tier] }}</el-descriptions-item>
        <el-descriptions-item label="状态">
          <span class="lic-status" :class="statusClass">{{ statusText }}</span>
        </el-descriptions-item>
        <el-descriptions-item label="客户">{{ snapshot.customer || '—' }}</el-descriptions-item>
        <el-descriptions-item label="授权编号">{{ snapshot.license_id || '—' }}</el-descriptions-item>
        <el-descriptions-item label="到期时间">{{ expiresText }}</el-descriptions-item>
        <el-descriptions-item label="产能配额">
          <div class="lic-quota">
            <span v-for="row in quotaRows" :key="row.label" class="lic-quota-item">
              {{ row.label }}：<b>{{ row.value }}</b>
            </span>
          </div>
        </el-descriptions-item>
        <!-- v2.0 配额使用量：已用 / 上限（剩余），超限项标红提示该升级档位 -->
        <el-descriptions-item v-if="quotaUsageRows.length" label="配额使用">
          <div class="lic-quota lic-quota-usage">
            <span v-for="row in quotaUsageRows" :key="row.label" class="lic-quota-item">
              {{ row.label }}：<b :class="row.over ? 'lic-quota-over' : undefined">{{ row.used }}</b>
              / {{ row.limit }}
              <span class="lic-quota-used">（剩余 {{ row.available }}）</span>
            </span>
          </div>
        </el-descriptions-item>
      </el-descriptions>
      <div v-else class="lic-empty">{{ loadError || '加载中...' }}</div>

      <!-- 配额降级告警：tier_quotas 漏配某档时配额被静默降到 absent 兜底，此处显式提示 -->
      <el-alert
        v-if="quotaSourceHint"
        type="warning"
        :title="quotaSourceHint"
        :closable="false"
        class="lic-quota-alert"
      />

      <!-- 存量配置超出当前档位配额：导入低档位 license 后最常见，必须显式告知 -->
      <el-alert
        v-if="quotaExceededHint"
        type="error"
        :title="quotaExceededHint"
        :closable="false"
        class="lic-quota-alert"
      />

      <!-- 导入授权文件 -->
      <h4 class="lic-section-title">导入授权文件</h4>
      <div class="lic-import-row">
        <el-button type="primary" :loading="importing" @click="fileInput?.click()">
          选择 .lic 文件导入
        </el-button>
        <input
          ref="fileInput"
          type="file"
          accept=".lic"
          class="lic-file-input"
          @change="onFileChange"
        />
        <span class="lic-import-hint">导入失败不会影响网关现有运行状态</span>
      </div>
      <el-alert
        v-if="importResult"
        :type="importResult.ok ? 'success' : 'error'"
        :title="importResult.msg"
        :closable="false"
        class="lic-alert"
      />

      <!-- 本机设备码 -->
      <h4 class="lic-section-title">本机设备码</h4>
      <p class="lic-hint">把设备码发给供应商签发授权文件，缺失项无需提供。</p>
      <div v-if="machineRows.length" class="lic-machine">
        <div v-for="row in machineRows" :key="row.label" class="lic-machine-row">
          <span class="lic-machine-label">{{ row.label }}</span>
          <span class="lic-machine-value">{{ row.value }}</span>
          <el-button text size="small" @click="copyText(row.value, row.label)">复制</el-button>
        </div>
        <el-button size="small" class="lic-copy-all" @click="copyAll">复制全部设备码</el-button>
      </div>
      <div v-else class="lic-empty">设备码未获取</div>
    </div>
    <template #footer>
      <el-button @click="$emit('update:modelValue', false)">关闭</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
/**
 * 授权管理弹窗
 *
 * 面向非技术客户的完整授权闭环（§16 授权管理界面）：
 *   1. 展示当前授权状态（档位 / 状态 / 客户 / 编号 / 到期时间 / v2.0 产能配额）；
 *   2. 本机设备码展示 + 一键复制（发给供应商签发）；
 *   3. 选择 .lic 文件导入，成功即时刷新快照与徽章，失败展示后端中文原因。
 *
 * 仅做展示与单次请求，无轮询副作用；快照打开时拉取一次。
 */
import { ref, computed, watch, onUnmounted } from 'vue'
import { ElMessage } from 'element-plus'
import { fetchLicense, importLicense, type LicenseSnapshot } from '@/api/license'
import { useTierStore, TIER_LABEL } from '../../stores/tier'
import { formatDate } from '@/utils/datetime'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'

defineProps<{ modelValue: boolean }>()
defineEmits<{ (e: 'update:modelValue', v: boolean): void }>()

const tierStore = useTierStore()
const { showToast } = useErrorDisplay()

// ===== 快照加载 =====
const loading = ref(false)
const snapshot = ref<LicenseSnapshot | null>(null)
const loadError = ref('')
let loadTimeout: ReturnType<typeof setTimeout> | null = null

/**
 * 2.5s 超时兜底：授权状态加载超过 2.5s 仍未返回时弹 warning 并关闭 loading，
 * 显示上次成功结果（若有）。真正成功返回后会清除此定时器。
 */
watch(loading, (val) => {
  if (val) {
    loadTimeout = setTimeout(() => {
      ElMessage.warning('授权状态加载超时，显示上次结果')
      loading.value = false
    }, 2500)
  } else if (loadTimeout) {
    clearTimeout(loadTimeout)
    loadTimeout = null
  }
})

// 对话框关闭/卸载时撤掉超时兜底，避免对已销毁组件写 loading 并弹出"加载超时"误报
onUnmounted(() => {
  if (loadTimeout) {
    clearTimeout(loadTimeout)
    loadTimeout = null
  }
})

async function onOpen() {
  importResult.value = null
  loading.value = true
  loadError.value = ''
  try {
    snapshot.value = await fetchLicense()
  } catch {
    loadError.value = '授权状态获取失败，请确认网关连接后重试'
  } finally {
    loading.value = false
  }
}

// ===== 状态展示 =====
const STATUS_TEXT: Record<string, string> = {
  ok: '正常',
  expiring: '即将到期',
  expired: '已过期',
  mismatch: '与本机不匹配',
  tampered: '时钟异常',
  invalid: '授权无效',
}
const statusText = computed(() => {
  const s = snapshot.value
  if (!s) return '—'
  return STATUS_TEXT[s.status] ?? s.status
})
const statusClass = computed(() => {
  const st = snapshot.value?.status
  if (st && st !== 'ok' && st !== 'expiring') return 'bad'
  return st === 'expiring' ? 'warn' : 'ok'
})
const expiresText = computed(() => {
  const s = snapshot.value
  if (!s) return '—'
  // ts-rs 导出 bigint，axios 实际解析为 number，Number() 双重兜底
  if (s.expires_at == null) return '永久有效'
  return formatDate(Number(s.expires_at) * 1000)
})

// ===== v2.0 产能配额展示（null = 不限）=====
const quotaRows = computed(() => {
  const q = snapshot.value?.quota
  const fmt = (v: number | bigint | null | undefined): string =>
    v == null ? '不限' : String(v)
  return [
    { label: '采集点数', value: fmt(q?.max_points) },
    { label: '并发协议', value: fmt(q?.max_protocols) },
    { label: '数据保留(天)', value: fmt(q?.storage_days) },
    { label: '上行通道数', value: fmt(q?.northbound_channels) },
  ]
})

// ===== v2.0 配额使用量（已用 / 上限 / 剩余）=====
interface QuotaUsageRow {
  label: string
  used: number
  limit: number
  available: number
  /** 已用量超过上限（存量配置超出当前档位配额）→ 标红提示升级 */
  over: boolean
}
const quotaUsageRows = computed<QuotaUsageRow[]>(() => {
  const s = snapshot.value
  if (!s) return []
  const usage = s.usage
  const avail = s.available
  // 旧版本网关不返回 usage / available → 不展示该行（不伪造数据）
  if (!usage || !avail) return []
  const rows: QuotaUsageRow[] = []
  const push = (
    label: string,
    limit: number | bigint | null | undefined,
    used: number,
    available: number | null | undefined,
  ) => {
    // 无上限（null）：没有"剩余"概念，不进使用量表
    if (limit == null) return
    const lim = Number(limit)
    rows.push({
      label,
      used,
      limit: lim,
      available: available == null ? Math.max(lim - used, 0) : Number(available),
      over: used > lim,
    })
  }
  push('采集点数', s.quota?.max_points, usage.points, avail.points)
  push('并发协议', s.quota?.max_protocols, usage.protocols, avail.protocols)
  push('数据保留(天)', s.quota?.storage_days, usage.storage_days, avail.storage_days)
  push('上行通道数', s.quota?.northbound_channels, usage.northbound_channels, avail.northbound_channels)
  return rows
})

// 存量配置超出当前档位配额 → 显式告警。
// 典型场景：导入低档位 license 后，原有 5000 点位配置仍在跑，但已超出新档位上限。
const quotaExceededHint = computed(() => {
  const over = quotaUsageRows.value.filter((r) => r.over)
  if (!over.length) return ''
  const items = over.map((r) => `${r.label} ${r.used}/${r.limit}`).join('，')
  return `当前配置已超出该档位配额（${items}），保存配置时会被拒绝，请升级档位或精简配置`
})

// 配额来源异常提示：tier_missing = 配置缺该档位，配额已降级到无 license 默认配额。
// absent_fallback 属预期（无 license / 授权失效），不打扰用户。
const quotaSourceHint = computed(() => {
  const s = snapshot.value
  if (!s) return ''
  if (s.quota_source === 'tier_missing') {
    return `配置未定义「${TIER_LABEL[s.tier]}」档位的默认配额，当前配额已降级到无授权默认配额。请在 config.json 的 license.tier_quotas 中补齐该档位。`
  }
  return ''
})

// ===== 设备码展示与复制 =====
interface MachineRow {
  label: string
  value: string
}

const machineRows = computed<MachineRow[]>(() => {
  const m = snapshot.value?.machine
  if (!m) return []
  const rows: MachineRow[] = []
  if (m.machine_key) rows.push({ label: '主设备码', value: m.machine_key })
  if (m.onboard_mac) rows.push({ label: '备用·板载网卡', value: m.onboard_mac })
  if (m.storage_serial) rows.push({ label: '备用·存储序列号', value: m.storage_serial })
  if (m.device_serial) rows.push({ label: '备用·设备序列号', value: m.device_serial })
  return rows
})

async function copyText(text: string, label: string) {
  try {
    await navigator.clipboard.writeText(text)
    ElMessage.success(`${label}已复制`)
  } catch {
    ElMessage.warning('复制失败，请手动选择文本复制')
  }
}

async function copyAll() {
  const main = machineRows.value.find((r) => r.label === '主设备码')
  const fallbacks = machineRows.value.filter((r) => r.label !== '主设备码')
  const lines: string[] = []
  if (main) lines.push(`主设备码: ${main.value}`)
  if (fallbacks.length) lines.push(`备用设备码: ${fallbacks.map((r) => r.value).join(',')}`)
  if (!lines.length) return
  try {
    await navigator.clipboard.writeText(lines.join('\n'))
    ElMessage.success('设备码已复制，请发送给供应商')
  } catch {
    ElMessage.warning('复制失败，请手动选择文本复制')
  }
}

// ===== .lic 导入 =====
const fileInput = ref<HTMLInputElement | null>(null)
const importing = ref(false)
const importResult = ref<{ ok: boolean; msg: string } | null>(null)

async function onFileChange(e: Event) {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = '' // 重置以允许重复选择同一文件
  if (!file) return
  importing.value = true
  importResult.value = null
  try {
    const content = await file.text()
    const snap = await importLicense(content)
    snapshot.value = snap
    importResult.value = { ok: true, msg: '授权导入成功，已即时生效' }
    // 刷新全局快照与顶栏徽章
    void tierStore.refreshFromLicense()
  } catch (err) {
    // 写操作全局弹窗被静音，页面自行提示：优先展示后端中文原因，缺失时退回动作文案
    showToast(err, '导入许可证失败')
    // 后端 400 返回面向客户的中文 message（签名/指纹/格式/IO），优先透传
    const msg =
      (err as { response?: { data?: { message?: string } } })?.response?.data?.message
      || '导入失败，请确认文件为供应商签发的 .lic 授权文件'
    importResult.value = { ok: false, msg }
  } finally {
    importing.value = false
  }
}
</script>

<style src="./LicenseDialog.css" />
