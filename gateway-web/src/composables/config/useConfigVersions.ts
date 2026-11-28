import { ref, onMounted } from 'vue'
import { ElMessage } from 'element-plus'
import { confirmAction } from '@/utils/ui/confirm'
import { logError } from '@/utils/logger'
import {
  listConfigVersions, getConfigVersion, rollbackConfigVersion, backupConfig,
  compareConfigVersions, type ConfigVersionMeta, type ConfigVersionSummary,
} from '@/api/uplink/config'
import { failureMessage } from '@/utils/error/errorHandler'
import { formatTimestamp } from '@/utils/time/datetime'
import { formatBytes } from '@/utils/time/format'
import { useRequestGuard } from '@/composables/core/useRequestGuard'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'

// GET 读操作的 catch 统一走 showToast：拦截器已对 GET 弹过全局通知并打
// USER_NOTIFIED 标记，showToast 据此跳过重复的那条；写操作（备份/回滚）才用
// failureMessage 保留带动作语境的本地提示（拦截器对写操作静音）。
const { showToast } = useErrorDisplay()

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
      showToast(e, '加载版本列表失败')
      logError('cfg-ver', e)
    } finally {
      loading.value = false
    }
  }

  function formatTime(t?: number) {
    // 配置版本时间戳约定为秒（旧实现无条件 ×1000），空值兜底 '-'
    return formatTimestamp(t, { unit: 'sec', fallback: '-' })
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
      showToast(e, '加载快照失败')
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
  // 版本对比请求序号守卫：旧对比结果不得覆盖新一轮对比
  const compareGuard = useRequestGuard()

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

  /**
   * 对比两个配置版本。
   *
   * 竞态守卫：连续触发两次版本对比时，先发的对比结果后到 → 展示的是上一次
   * 的两版本 diff，而 compareFrom/compareTo 却是新选的，差异内容与标题不符。
   */
  async function runCompare() {
    if (compareFrom.value == null || compareTo.value == null) {
      ElMessage.warning('请选择两个版本')
      return
    }
    if (compareFrom.value === compareTo.value) {
      ElMessage.warning('请选择两个不同的版本')
      return
    }
    const seq = compareGuard.next()
    compareLoading.value = true
    try {
      const [result, fromCfg, toCfg] = await Promise.all([
        compareConfigVersions(compareFrom.value, compareTo.value),
        getConfigVersion(compareFrom.value),
        getConfigVersion(compareTo.value),
      ])
      if (!compareGuard.isCurrent(seq)) return
      compareResult.value = result
      compareFromJson.value = JSON.stringify(fromCfg, null, 2)
      compareToJson.value = JSON.stringify(toCfg, null, 2)
    } catch (e) {
      if (compareGuard.isCurrent(seq)) {
        showToast(e, '对比版本失败')
        logError('cfg-ver', e)
      }
    } finally {
      if (compareGuard.isCurrent(seq)) compareLoading.value = false
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
    formatTime, formatBytes, fieldLabel,
  }
}
