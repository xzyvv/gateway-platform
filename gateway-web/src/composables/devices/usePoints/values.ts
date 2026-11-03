import { ElMessage } from 'element-plus'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import type { PointConfig } from '@/api/types'

/**
 * 点位当前值读取
 *
 * 手动读一次点位实时值：成功显示当前值，无有效值时提示采集状态（如 status=timeout），
 * 便于区分「点位没有值」与「点位采集异常」。
 */
export function createPointValues() {
  const { readOnce } = useReadPointNow()

  async function readPoint(row: PointConfig) {
    const outcome = await readOnce(row.id)
    if (!outcome.ok) {
      ElMessage.warning(`点位 ${row.id} 暂无有效值（${outcome.error}）`)
      return
    }
    ElMessage.success(`点位 ${row.id} 当前值: ${outcome.value}`)
  }

  return { readPoint }
}
