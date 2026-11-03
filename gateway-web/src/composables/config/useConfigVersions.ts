import { ref, onMounted } from 'vue'
import { ElMessage } from 'element-plus'
import { confirmAction } from '@/utils/confirm'
import { logError } from '@/utils/logger'
import {
  listConfigVersions, getConfigVersion, rollbackConfigVersion, backupConfig,
  compareConfigVersions, type ConfigVersionMeta, type ConfigVersionSummary,
} from '@/api/config'
import { failureMessage } from '@/utils/errorHandler'
import { formatDateTime } from '@/utils/datetime'

/** 字段中文标签映射（配置对比时使用） */
const FIELD_LABELS: Record<string, string> = {
  buses: '总线配置',
  points: '点位配置',
  devices: '设备配置',
  mqtt: 'MQTT 通道',
  web: 'Web 配置',
  ntp: 'NTP 配置',
}

/** 配置字段名转中文标签（无映射时回退为原始字段名），供配置对比类组件复用 */
export function fieldLabel(f: string) {
  return FIELD_LABELS[f] || f
}

/**
 * ConfigVersions 页面数据层封装
 *
 * 把 ConfigVersions.vue 中的版本列表、快照查看、版本对比、回滚、
 * 字段标签、格式化函数集中到一个 composable，主组件只剩 template。
 */
export function useConfigVersions() {
  // 后端返回元数据字段：id / created_at / message / size / is_current
  const versions = ref<ConfigVersionSummary[]>([])
  const loading = ref(false)

  async function load() {
    loading.value = true
    try {
      versions.value = await listConfigVersions()
    } catch (e) {
      ElMessage.error(failureMessage('加载版本列表', e))
      logError('cfg-ver', e)
    } finally {
      loading.value = false
    }
  }

  function formatTime(t?: number) {
    if (!t) return '-'
    return formatDateTime(t * 1000)
  }

  function formatSize(bytes: number) {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    return `${(bytes / 1024 / 1024).toFixed(2)} MB`
  }

  // 快照查看
  const showSnapshot = ref(false)
  const snapshotVersion = ref<ConfigVersionSummary | null>(null)
  const snapshotJson = ref('')
  const snapshotLoading = ref(false)

  async function viewSnapshot(v: ConfigVersionSummary) {
    snapshotVersion.value = v
    showSnapshot.value = true
    snapshotJson.value = ''
    snapshotLoading.value = true
    try {
      const cfg = await getConfigVersion(v.id)
      snapshotJson.value = JSON.stringify(cfg, null, 2)
    } catch (e) {
      ElMessage.error(failureMessage('加载快照', e))
      logError('cfg-ver', e)
    } finally {
      snapshotLoading.value = false
    }
  }

  // 版本对比
  const showCompare = ref(false)
  const compareFrom = ref<number | undefined>(undefined)
  const compareTo = ref<number | undefined>(undefined)
  const compareResult = ref<ConfigVersionMeta | null>(null)
  const compareFromJson = ref('')
  const compareToJson = ref('')
  const compareLoading = ref(false)

  function openCompare() {
    if (versions.value.length < 2) {
      ElMessage.warning('至少需要 2 个版本才能对比')
      return
    }
    compareFrom.value = versions.value[versions.value.length - 1]?.id
    compareTo.value = versions.value[0]?.id
    compareResult.value = null
    compareFromJson.value = ''
    compareToJson.value = ''
    showCompare.value = true
  }

  async function compareWithCurrent(v: ConfigVersionSummary) {
    const current = versions.value.find((x) => x.is_current) || versions.value[0]
    if (!current || current.id === v.id) {
      ElMessage.warning('该版本即当前版本，无可对比项')
      return
    }
    compareFrom.value = v.id
    compareTo.value = current.id
    compareResult.value = null
    showCompare.value = true
    await runCompare()
  }

  async function runCompare() {
    if (compareFrom.value == null || compareTo.value == null) {
      ElMessage.warning('请选择两个版本')
      return
    }
    if (compareFrom.value === compareTo.value) {
      ElMessage.warning('请选择两个不同的版本')
      return
    }
    compareLoading.value = true
    try {
      const [result, fromCfg, toCfg] = await Promise.all([
        compareConfigVersions(compareFrom.value, compareTo.value),
        getConfigVersion(compareFrom.value),
        getConfigVersion(compareTo.value),
      ])
      compareResult.value = result
      compareFromJson.value = JSON.stringify(fromCfg, null, 2)
      compareToJson.value = JSON.stringify(toCfg, null, 2)
    } catch (e) {
      ElMessage.error(failureMessage('对比版本', e))
      logError('cfg-ver', e)
    } finally {
      compareLoading.value = false
    }
  }

  // 手动备份
  const backingUp = ref(false)
  async function backup() {
    backingUp.value = true
    try {
      const r = await backupConfig()
      ElMessage.success(`已创建备份 v${r.version_id}（${r.points} 点位 / ${r.devices} 设备）`)
      await load()
    } catch (e) {
      ElMessage.error(failureMessage('手动备份', e))
      logError('cfg-ver-backup', e)
    } finally {
      backingUp.value = false
    }
  }

  // 回滚
  async function rollback(v: ConfigVersionSummary) {
    if (!(await confirmAction({
      message: `确认回滚到版本 v${v.id}？当前配置将被覆盖，此操作会生成新版本记录。`,
      title: '回滚确认',
    }))) return
    try {
      await rollbackConfigVersion(v.id)
      ElMessage.success(`已回滚到 v${v.id}`)
      await load()
    } catch (e) {
      ElMessage.error(failureMessage('回滚', e))
      logError('cfg-ver', e)
    }
  }

  onMounted(load)

  return {
    // state
    versions, loading, backingUp,
    showSnapshot, snapshotVersion, snapshotJson, snapshotLoading,
    showCompare, compareFrom, compareTo, compareResult,
    compareFromJson, compareToJson, compareLoading,
    // actions
    load, backup, viewSnapshot, openCompare, compareWithCurrent, runCompare, rollback,
    // formatters
    formatTime, formatSize, fieldLabel,
  }
}
