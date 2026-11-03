import { h, reactive, ref } from 'vue'
import { ElMessage, ElNotification } from 'element-plus'
import { createPoint } from '@/api/points'
import { parseImportPoints } from '@/utils/importPoints'
import { mapPointToTarget } from '@/utils/deviceCopy'
import { clampInterval } from './validation'
import type { BatchImportResult, PointFailure, PointsState } from './types'

/**
 * 批量导入点位
 *
 * 解析粘贴的 JSON → 归一到目标设备 → 并发（上限 8）逐条创建，
 * 并实时回报进度（成功/失败都计入完成数），部分失败用常驻通知列出明细。
 */
export function createPointBatchImport(state: PointsState, loadPoints: () => Promise<void>) {
  const { devices, selectedDevice } = state

  const showImport = ref(false)
  const importText = ref('')
  const importing = ref(false)
  /**
   * 批量导入实时进度：每完成一条（成功或失败）completed++。
   * 结束后保留 completed/total 供导入弹窗展示，下一次导入开始时重置。
   */
  const importProgress = reactive({ completed: 0, total: 0, running: false })

  async function doImport(): Promise<BatchImportResult | undefined> {
    const { valid, error } = parseImportPoints(importText.value)
    if (error) {
      ElMessage.error(error)
      return
    }
    if (valid.length === 0) {
      ElMessage.warning('未解析到任何有效点位，请检查 JSON 格式')
      return
    }
    const target = selectedDevice.value
    const targetDevice = devices.value.find((d) => d.id === target)
    if (!targetDevice) {
      ElMessage.error('请先选择目标设备')
      return
    }
    // 点位必须继承目标设备的总线/协议：后端 validate_point_inherits_device 会逐条拒绝
    // 不一致的点位，而粘贴「从其它设备导出」的 JSON 时原 bus/protocol 几乎必然不同。
    // 旧实现只改 device_id，导致跨设备导入全部失败（第 84 行只剩一句「导入失败」）。
    // 同时保留整份点位字段：旧实现只挑 8 个字段，name/scale/unit/data_type 全丢，
    // 导入后点位变成无名、量程失效的空壳。
    const payloads = valid.map((p) => ({
      ...mapPointToTarget(p, target),
      bus: targetDevice.bus,
      protocol: targetDevice.protocol,
      interval: clampInterval(p.interval),
    }))
    importing.value = true
    importProgress.total = payloads.length
    importProgress.completed = 0
    importProgress.running = true
    const queue = [...payloads]
    let created = 0
    const failed: PointFailure[] = []
    const CONCURRENCY = 8
    try {
      async function worker() {
        while (!state.isUnmounted()) {
          const p = queue.shift()
          if (!p) break
          try {
            await createPoint(target, p)
            created++
          } catch (e) {
            failed.push({ id: p.id, reason: e instanceof Error ? e.message : String(e || '未知错误') })
          } finally {
            importProgress.completed++
          }
        }
      }
      await Promise.all(Array.from({ length: Math.min(CONCURRENCY, payloads.length) }, () => worker()))
      if (state.isUnmounted()) return
      const total = payloads.length
      if (failed.length === 0) {
        ElMessage.success(`成功导入 ${created} 个点位`)
      } else if (created === 0) {
        ElMessage.error(`导入失败：${failed[0]?.reason ?? '未知错误'}`)
      } else {
        ElMessage.success(`成功导入 ${created}/${total} 个点位`)
        // 部分失败：常驻通知列出失败明细（同 batchDelete 风格）
        ElNotification({
          type: 'warning',
          title: `批量导入：${failed.length} 个点位未创建`,
          duration: 0,
          message: h('div', [
            h('p', { style: 'margin:0 0 6px;font-size: 0.75rem;color:var(--text-secondary)' },
              `成功 ${created} 个，失败 ${failed.length} 个。以下点位仍需手动处理：`),
            ...failed.slice(0, 8).map((f) =>
              h('div', { style: 'font-size: 0.75rem;line-height:1.7;word-break:break-all' },
                `${f.id} — ${f.reason}`)),
            failed.length > 8
              ? h('div', { style: 'font-size: 0.75rem;color:var(--text-secondary);margin-top:4px' },
                `其余 ${failed.length - 8} 条略`)
              : null,
          ]),
        })
      }
      if (created > 0) {
        showImport.value = false
        importText.value = ''
        loadPoints()
      }
    } finally {
      importProgress.running = false
      if (!state.isUnmounted()) importing.value = false
    }
    return { created, failed }
  }

  return { showImport, importText, importing, importProgress, doImport }
}
