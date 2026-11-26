/**
 * 参数探测页状态机（从 AutoDetect.vue 拆出）
 *
 * 负责：启动/中止探测、轮询进度（status 非 running 即收尾）、成功/失败参数
 * 汇总、以及把成功参数整理成可复制的纯文本。页面组件只保留布局与展示。
 *
 * 轮询与收尾统一由 usePolledTask 负责（DD-49）：status 非 running 即收尾、
 * 连续 3 次失败停表、定时器单例与卸载清理沿用底层 usePolling。
 */
import { computed, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { startAutoDetect, getAutoDetectResult, stopAutoDetect, type DetectStep, type AutoDetectResult } from '@/api/protocolTools'
import { usePolledTask } from '@/composables/core/useTaskProgress'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import { useProtocolCaps } from '@/composables/acquisition/useProtocolCaps'
import { copyText } from '@/utils/clipboard'

const POLL_INTERVAL_MS = 500

export function useAutoDetect() {
  // 共享调试目标（审查#5）：设备选择跨调试页保持一致，选项加载后失效值自动回退第一台
  const { devices, deviceOptions, loading: targetsLoading, selectedDeviceId: deviceId } = useDebugTargets()
  // B3：能力位门控（注册表驱动）——只有声明 supports_auto_detect 的协议才允许参数探测，
  // 元数据缺失时按不支持处理，避免给出「点了必然失败」的按钮。
  const { loaded: capsLoaded, failed: capsFailed, capsOf } = useProtocolCaps()
  const selectedProtocol = computed(
    () => devices.value.find((d) => d.id === deviceId.value)?.protocol ?? '',
  )
  const autoDetectSupported = computed(
    () => capsOf(selectedProtocol.value)?.supports_auto_detect ?? false,
  )
  const { showToast } = useErrorDisplay()

  const detecting = ref(false)
  const done = ref(false)
  const steps = ref<DetectStep[]>([])
  const progress = ref(0)
  const successSteps = computed(() => steps.value.filter((s) => s.status === 'success'))
  const failedSteps = computed(() => steps.value.filter((s) => s.status === 'fail'))

  let activeDetectId = ''

  // DD-49：轮询与终态收尾交给统一口径（DD-02 的「非 running 即收尾」只保留这一处）
  const { begin: beginPoll, end: endPoll } = usePolledTask<AutoDetectResult>({
    interval: POLL_INTERVAL_MS,
    fetch: () => getAutoDetectResult(activeDetectId),
    statusOf: (r) => r.status,
    // 结果不存在（如网关重启丢了内存态任务）按失败处理，交给连续失败上限收敛
    requireResult: true,
    onUpdate: (r) => {
      steps.value = r.steps
      progress.value = r.progress
    },
    onSettled: (_r, terminal) => {
      detecting.value = false
      done.value = true
      if (terminal === 'aborted') ElMessage.info('探测已中止')
      else if (terminal === 'failed') ElMessage.error('探测中断：总线掉线')
      else if (failedSteps.value.length) ElMessage.warning(`探测完成，${failedSteps.value.length} 项失败`)
      else ElMessage.success('探测完成')
    },
    onFetchFailures: (e) => {
      // 网关/总线掉线时轮询会持续失败：达到上限即停，避免页面永远停在"探测中"
      detecting.value = false
      showToast(e, '获取探测进度失败')
    },
  })

  async function detect() {
    if (!deviceId.value) { ElMessage.warning('请先选择目标设备'); return }
    if (!autoDetectSupported.value) { ElMessage.warning('当前设备协议不支持参数探测'); return }
    endPoll()
    detecting.value = true; done.value = false; steps.value = []; progress.value = 0
    try {
      activeDetectId = await startAutoDetect(deviceId.value)
      beginPoll(true)
    } catch (e) {
      detecting.value = false
      endPoll()
      showToast(e, '启动探测失败')
    }
  }

  /** 中止探测：后端在下一探测项前停止并释放总线租约，结果由轮询收敛 */
  async function stopDetect() {
    if (!activeDetectId) return
    try { await stopAutoDetect(activeDetectId) } catch { /* 轮询会收敛，无需提示 */ }
  }

  /** 探测参数整理成纯文本：现场贴进工单/发给设备厂商核对 */
  function copyParams() {
    const dev = deviceOptions.value.find((o) => o.value === deviceId.value)?.label ?? deviceId.value
    const lines = [`设备: ${dev}`, ...successSteps.value.map((s) => `${s.param}: ${s.value}`)]
    void copyText(lines.join('\n'), '探测参数已复制')
  }

  return {
    deviceOptions,
    targetsLoading,
    deviceId,
    capsLoaded,
    capsFailed,
    selectedProtocol,
    autoDetectSupported,
    detecting,
    done,
    steps,
    progress,
    successSteps,
    failedSteps,
    detect,
    stopDetect,
    copyParams,
  }
}
