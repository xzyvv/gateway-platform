/**
 * 批量导出 · 点位配置（CSV / JSON）
 *
 * 两个格式对应批量导入页的两条回灌路径，导出后可直接改完再导回：
 * - CSV：10 列表头与「批量导入」模板完全一致，适合 Excel 批量改地址/变比。
 *   注意导入侧只读这 10 列，其余字段（scale/unit/data_type/writable 等）
 *   会被补默认值 —— 需要完整保真请用 JSON。
 * - JSON：`PointConfig[]` 原样快照，可整体粘贴到批量导入页的「粘贴 JSON」。
 *
 * 导出范围是**配置点位**（后端 `GET /api/point-tools/export` 读 `cfg.points`），
 * 不含模板设备展开出来的点位：展开点位不在配置里，导回去会被固化成显式点位，
 * 运行期与模板展开结果重复。
 */
import { ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { exportPoints } from '@/api/points'
import { downloadTableCsv, triggerDownload } from '@/utils/exportCsv'

/** 与批量导入 CSV 模板逐列一致（见 views/devices/batchImport/pointCsv.ts） */
const EXPORT_CSV_HEADER = [
  'id', 'name', 'device_id', 'bus', 'protocol', 'address',
  'slave_id', 'address_num', 'ct_ratio', 'pt_ratio',
]

/** 导出格式：CSV（Excel 编辑）/ JSON（完整保真） */
export type PointsExportFormat = 'csv' | 'json'

/** CSV 导出的有损风险提示（导入侧只读这 10 列，其余字段会被补默认值） */
const CSV_LOSSY_WARNING =
  'CSV 只含 id / 名称 / 设备 / 总线 / 协议 / 地址 / 从站 / 地址偏移 / CT / PT 共 10 列。'
  + '改完导回时，scale、offset、unit、data_type、writable、公式等其余字段会被重置为默认值。'
  + '需要完整保真请改用「导出 JSON」。'

/** 导出中的状态 + 触发入口 */
export function createPointExport() {
  const exporting = ref(false)

  /**
   * CSV 导出前二次确认。
   *
   * @returns true = 用户确认继续；false = 取消（含直接关闭弹窗）
   */
  async function confirmCsvLossy(): Promise<boolean> {
    try {
      await ElMessageBox.confirm(CSV_LOSSY_WARNING, '导出 CSV 前请确认', {
        type: 'warning',
        confirmButtonText: '继续导出',
        cancelButtonText: '取消',
      })
      return true
    } catch {
      return false // 用户取消
    }
  }

  /** @param format 下拉菜单回传的是字符串，非法值直接忽略 */
  async function doExport(format: string): Promise<void> {
    if (format !== 'csv' && format !== 'json') return
    if (exporting.value) return
    if (format === 'csv' && !(await confirmCsvLossy())) return
    exporting.value = true
    try {
      const { points } = await exportPoints()
      if (!points || points.length === 0) {
        ElMessage.warning('没有可导出的配置点位（模板设备展开出的点位不在导出范围）')
        return
      }
      const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')
      if (format === 'csv') {
        const rows = points.map((p) => [
          p.id, p.name ?? '', p.device_id ?? '', p.bus, p.protocol, p.address,
          p.slave_id ?? '', p.address_num ?? '', p.ct_ratio ?? '', p.pt_ratio ?? '',
        ])
        // 走统一出口：公式注入防护 + RFC 4180 转义 + UTF-8 BOM
        downloadTableCsv([...EXPORT_CSV_HEADER], rows, `points-${stamp}.csv`)
      } else {
        triggerDownload(
          new Blob([JSON.stringify(points, null, 2)], { type: 'application/json' }),
          `points-${stamp}.json`,
        )
      }
      ElMessage.success(`已导出 ${points.length} 个点位`)
    } catch (e) {
      ElMessage.error(`导出失败：${e instanceof Error ? e.message : '未知错误'}`)
    } finally {
      exporting.value = false
    }
  }

  return { exporting, doExport }
}
