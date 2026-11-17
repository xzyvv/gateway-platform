/**
 * 批量导出 · 点位配置（CSV 精简 / CSV 完整 / JSON）
 *
 * 三种格式对应批量导入页的三条回灌路径，导出后可直接改完再导回：
 * - CSV 精简档：10 列，与「批量导入」的精简模板逐列一致，Excel 里改地址/变比最顺手。
 *   配合导入页的**合并**模式，导回时未出现的列保留原值，不丢 scale/unit/公式等字段。
 * - CSV 完整档：全部可平铺字段（含 scale、unit、公式、告警阈值、可写标记等），
 *   用于「导出 → 改 → 原样导回」的闭环。聚合配置是嵌套结构，CSV 装不下，不在其中。
 * - JSON：`PointConfig[]` 原样快照，可整体粘贴到批量导入页的「粘贴 JSON」。
 *
 * 导出范围是**配置点位**（后端 `GET /api/point-tools/export` 读 `cfg.points`），
 * 不含模板设备展开出来的点位：展开点位不在配置里，导回去会被固化成显式点位，
 * 运行期与模板展开结果重复。
 */
import { ref } from 'vue'
import { ElMessage } from 'element-plus'
import { exportPoints } from '@/api/points'
import { confirmAction } from '@/utils/confirm'
import { downloadTableCsv, fileTimestamp, triggerDownload } from '@/utils/exportCsv'
import {
  POINT_FIELDS_BASIC,
  POINT_FIELDS_FULL,
  formatPointCell,
  type PointFieldSpec,
} from '@/utils/pointFields'

/** 导出格式：精简 CSV / 完整 CSV / JSON 快照 */
export type PointsExportFormat = 'csv-basic' | 'csv-full' | 'json'

/** 各档位的字段清单与列语义说明 */
const CSV_PRESETS: Record<'csv-basic' | 'csv-full', { fields: PointFieldSpec[]; hint: string }> = {
  'csv-basic': {
    fields: POINT_FIELDS_BASIC,
    hint: '精简 10 列（id / 名称 / 设备 / 总线 / 协议 / 地址 / 从站 / 地址偏移 / CT / PT）。'
      + '导回时若选「合并」模式，未出现的列保留原值；出现的列留空表示清空该字段。',
  },
  'csv-full': {
    fields: POINT_FIELDS_FULL,
    hint: '完整字段导出（含 scale、offset、unit、公式、告警阈值、可写标记等）。'
      + '导回时同样是按列覆盖，出现的列留空表示清空该字段；聚合配置是嵌套结构，不在 CSV 内。',
  },
}

/** 导出中的状态 + 触发入口 */
export function createPointExport() {
  const exporting = ref(false)

  /**
   * CSV 导出前的语义说明。
   *
   * @returns true = 用户确认继续；false = 取消（含直接关闭弹窗）
   */
  async function confirmCsvSemantics(format: 'csv-basic' | 'csv-full'): Promise<boolean> {
    return confirmAction({
      message: CSV_PRESETS[format].hint,
      title: '导出 CSV 说明',
      type: 'info',
      confirmText: '继续导出',
    })
  }

  /** @param format 下拉菜单回传的是字符串，非法值直接忽略 */
  async function doExport(format: string): Promise<void> {
    if (format !== 'csv-basic' && format !== 'csv-full' && format !== 'json') return
    if (exporting.value) return
    if (format !== 'json' && !(await confirmCsvSemantics(format))) return
    exporting.value = true
    try {
      const { points } = await exportPoints()
      if (!points || points.length === 0) {
        ElMessage.warning('没有可导出的配置点位（模板设备展开出的点位不在导出范围）')
        return
      }
      const stamp = fileTimestamp()
      if (format === 'json') {
        triggerDownload(
          new Blob([JSON.stringify(points, null, 2)], { type: 'application/json' }),
          `points-${stamp}.json`,
        )
      } else {
        const { fields } = CSV_PRESETS[format]
        const rows = points.map((p) => fields.map((f) => formatPointCell(f, p[f.key])))
        // 走统一出口：公式注入防护 + RFC 4180 转义 + UTF-8 BOM
        downloadTableCsv(fields.map((f) => f.column), rows, `points-${stamp}.csv`)
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
