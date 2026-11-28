import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPointExport } from './export'

// 只打桩本文件用到的两个出口：接口与下载工具
const apiState = vi.hoisted(() => ({
  points: [] as Array<Record<string, unknown>>,
  fail: null as Error | null,
}))

vi.mock('@/api/acquisition/points', () => ({
  exportPoints: vi.fn(async () => {
    if (apiState.fail) throw apiState.fail
    return { total: apiState.points.length, points: apiState.points }
  }),
}))

const downloadState = vi.hoisted(() => ({
  csv: vi.fn(),
  blob: vi.fn(),
}))

vi.mock('@/utils/csv/exportCsv', () => ({
  downloadTableCsv: downloadState.csv,
  triggerDownload: downloadState.blob,
  // 导出文件名的时间戳由 utils/exportCsv 统一生成（本地时间，非 UTC）；
  // 这里给固定值，文件名断言就不受运行机器时区影响
  fileTimestamp: () => '2026-11-17-21-00-00',
}))

const messageState = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
}))

const confirmState = vi.hoisted(() => ({
  confirm: vi.fn(async () => 'confirm'),
}))

vi.mock('element-plus', () => ({
  ElMessage: messageState,
  ElMessageBox: confirmState,
}))

/** 与「批量导入」CSV 模板逐列一致 */
const CSV_HEADER = [
  'id', 'name', 'device_id', 'bus', 'protocol', 'address',
  'slave_id', 'address_num', 'ct_ratio', 'pt_ratio',
]

describe('createPointExport（点位批量导出）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    apiState.fail = null
    apiState.points = [
      {
        id: 'p1', name: '电压', device_id: 'd1', bus: 'bus1', protocol: 'modbus',
        address: '40001', slave_id: 1, address_num: 0, ct_ratio: 1, pt_ratio: 1,
      },
      { id: 'p2', name: '电流', device_id: '', bus: 'bus2', protocol: 'dlt645', address: '02010100' },
    ]
  })

  it('CSV 精简档：表头与导入模板一致，缺失字段留空', async () => {
    const { doExport, exporting } = createPointExport()
    await doExport('csv-basic')

    expect(exporting.value).toBe(false)
    expect(confirmState.confirm).toHaveBeenCalledTimes(1)
    expect(downloadState.csv).toHaveBeenCalledTimes(1)
    const [header, rows, filename] = downloadState.csv.mock.calls[0] as [string[], unknown[][], string]
    expect(header).toEqual(CSV_HEADER)
    expect(rows).toHaveLength(2)
    expect(rows[0]).toEqual(['p1', '电压', 'd1', 'bus1', 'modbus', '40001', '1', '0', '1', '1'])
    // 可选字段缺失时补空串，保证列数与表头对齐
    expect(rows[1]).toEqual(['p2', '电流', '', 'bus2', 'dlt645', '02010100', '', '', '', ''])
    expect(filename).toMatch(/^points-.+\.csv$/)
    expect(messageState.success).toHaveBeenCalledWith('已导出 2 个点位')
  })

  it('CSV 完整档：带 scale / unit / 公式等列，列名与导入模板一致', async () => {
    apiState.points = [
      {
        id: 'p1', name: '电压', device_id: 'd1', bus: 'bus1', protocol: 'modbus',
        address: '40001', slave_id: 1, address_num: 0, scale: 0.1, offset: 2,
        unit: 'kV', formula: 'a * 2', interval: 10, writable: true, on_change: true,
      },
    ]
    const { doExport } = createPointExport()
    await doExport('csv-full')

    const [header, rows] = downloadState.csv.mock.calls[0] as [string[], unknown[][]]
    expect(header).toContain('scale')
    expect(header).toContain('unit')
    expect(header).toContain('formula')
    expect(header).toContain('writable')
    // 聚合配置是嵌套结构，CSV 装不下，不在列里
    expect(header).not.toContain('aggregation')
    expect(rows[0]).toContain('0.1')
    expect(rows[0]).toContain('kV')
    expect(rows[0]).toContain('a * 2')
    expect(rows[0]).toContain('true')
  })

  it('JSON：整体快照，文件名带 .json，不弹有损警告', async () => {
    const { doExport } = createPointExport()
    await doExport('json')

    expect(confirmState.confirm).not.toHaveBeenCalled()
    expect(downloadState.csv).not.toHaveBeenCalled()
    expect(downloadState.blob).toHaveBeenCalledTimes(1)
    const [blob, filename] = downloadState.blob.mock.calls[0] as [Blob, string]
    expect(blob).toBeInstanceOf(Blob)
    expect(filename).toMatch(/^points-.+\.json$/)
    expect(messageState.success).toHaveBeenCalledWith('已导出 2 个点位')
  })

  it('CSV：用户在警告弹窗点取消则不导出', async () => {
    confirmState.confirm.mockRejectedValueOnce(new Error('cancel'))
    const { doExport, exporting } = createPointExport()
    await doExport('csv-basic')

    expect(exporting.value).toBe(false)
    expect(downloadState.csv).not.toHaveBeenCalled()
    expect(messageState.success).not.toHaveBeenCalled()
    expect(messageState.error).not.toHaveBeenCalled()
  })

  it('没有配置点位时只提示、不落文件', async () => {
    apiState.points = []
    const { doExport } = createPointExport()
    await doExport('csv-basic')

    expect(downloadState.csv).not.toHaveBeenCalled()
    expect(downloadState.blob).not.toHaveBeenCalled()
    expect(messageState.warning).toHaveBeenCalledTimes(1)
  })

  it('接口失败时提示错误且不落文件', async () => {
    apiState.fail = new Error('boom')
    const { doExport, exporting } = createPointExport()
    await doExport('csv-basic')

    expect(exporting.value).toBe(false)
    expect(downloadState.csv).not.toHaveBeenCalled()
    expect(messageState.error).toHaveBeenCalledWith('导出失败：boom')
  })

  it('非法格式直接忽略', async () => {
    const { doExport } = createPointExport()
    await doExport('xlsx')

    expect(downloadState.csv).not.toHaveBeenCalled()
    expect(downloadState.blob).not.toHaveBeenCalled()
  })
})
