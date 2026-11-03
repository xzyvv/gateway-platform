/**
 * 部署向导 · 第 5 步（验证采集）的状态与真实接口调用。
 *
 * 逐点调用真实读取接口，把成功 / 失败如实列出，不做「假成功」兜底。
 * 这里持有验证相关的全部响应式状态，页面只把状态透传给子组件渲染。
 *
 * 典型应用场景：部署向导的最后一步（验证采集）。
 */
import { ref, onUnmounted } from 'vue'
import { ElMessage } from 'element-plus'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import type { PointConfig } from '@/api/types'
import type { ReadResult } from './types'

export function useVerifyStep() {
  const { readSequential } = useReadPointNow()
  /** 采集测试进行中 */
  const verifying = ref(false)
  /** 至少有一个点位读取成功 */
  const verified = ref(false)
  /** 测试结论文案 */
  const verifySummary = ref('')
  /** 是否有失败点位 */
  const verifyFailed = ref(false)
  /** 逐点读取结果 */
  const readResults = ref<ReadResult[]>([])

  /** 组件卸载后不再继续写状态 */
  let unmounted = false
  onUnmounted(() => {
    unmounted = true
  })

  /** 逐点执行采集测试 */
  async function verify(points: PointConfig[]) {
    if (!points.length) {
      ElMessage.warning('没有可读的点位')
      return
    }
    verifying.value = true
    verified.value = false
    readResults.value = []
    verifySummary.value = ''
    let ok = 0
    const byId = new Map(points.map((p) => [p.id, p]))
    await readSequential(
      points.map((p) => p.id),
      {
        shouldStop: () => unmounted,
        onResult: (outcome) => {
          const p = byId.get(outcome.pointId)
          if (outcome.ok) ok++
          const name = p ? p.name || p.id : outcome.pointId
          readResults.value.push({
            name,
            id: outcome.pointId,
            ok: outcome.ok,
            value: outcome.ok ? String(outcome.value) : outcome.error || '无响应',
          })
        },
      },
    )
    if (unmounted) return
    const failed = points.length - ok
    verifyFailed.value = failed > 0
    verifySummary.value = failed
      ? `采集测试完成：成功 ${ok} 个，失败 ${failed} 个。失败项请核对从站地址、寄存器地址与数据类型。`
      : `采集测试完成：${ok} 个点位全部读取成功。`
    verified.value = ok > 0
    verifying.value = false
    if (failed) ElMessage.warning('部分点位读取失败，请按提示核对配置')
    else ElMessage.success('所有点位读取成功')
  }

  return {
    verifying,
    verified,
    verifySummary,
    verifyFailed,
    readResults,
    verify,
  }
}
