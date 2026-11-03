import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 与 triggerDownload 经 vi.hoisted 提前声明，供 vi.mock 工厂引用
const { requestMock, triggerDownloadMock } = vi.hoisted(() => ({
  requestMock: vi.fn(),
  triggerDownloadMock: vi.fn(),
}))
vi.mock('./index', () => ({ request: requestMock }))
vi.mock('@/utils/exportCsv', () => ({ triggerDownload: triggerDownloadMock }))

import { downloadArchive, listArchives, viewArchive } from './logArchive'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('logArchive 日志归档契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
    triggerDownloadMock.mockReset()
  })

  // 锁定归档列表路径：GET /api/diagnostics/logs/archives，
  // 并把 { archives } 包裹体解开成数组（页面直接 v-for 遍历）。
  it('listArchives 走 GET /api/diagnostics/logs/archives 并把 { archives } 解开为数组', async () => {
    const archives = [{ date: '2026-10-30', size_bytes: 1024, current: false }]
    requestMock.mockResolvedValue({ archives, count: 1 })

    await expect(listArchives()).resolves.toEqual(archives)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/diagnostics/logs/archives')
  })

  // 边界：响应缺 archives 字段时回退空数组，避免页面拿到 undefined 崩溃
  it('listArchives 在响应缺少 archives 时回退空数组', async () => {
    requestMock.mockResolvedValue({})
    await expect(listArchives()).resolves.toEqual([])
  })

  // 锁定查看路径与默认行数：GET /api/diagnostics/logs/download?lines=200&date=...
  // 该端点是裸文本响应（不走 ApiResponse 信封解包）。
  it('viewArchive 走 GET /api/diagnostics/logs/download，默认取 200 行并带 date', async () => {
    requestMock.mockResolvedValue('')

    await viewArchive('2026-10-30')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/diagnostics/logs/download?lines=200&date=2026-10-30')
  })

  // 边界：可指定行数，行数必须拼进 lines 查询参数
  it('viewArchive 支持自定义行数', async () => {
    requestMock.mockResolvedValue('')

    await viewArchive('2026-10-30', 50)

    expect(firstCallConfig().url).toBe('/api/diagnostics/logs/download?lines=50&date=2026-10-30')
  })

  // 锁定 JSON 日志行解析：timestamp→time、level 转大写、target、fields.message→message
  it('viewArchive 解析 JSON 日志行（level 转大写、message 取 fields.message）', async () => {
    const raw = JSON.stringify({
      timestamp: '2026-10-30T01:02:03Z',
      level: 'warn',
      target: 'mqtt',
      fields: { message: '连接重试中' },
    })
    requestMock.mockResolvedValue(raw)

    const view = await viewArchive('2026-10-30')

    expect(view.date).toBe('2026-10-30')
    expect(view.lines).toHaveLength(1)
    expect(view.lines[0]!.time).toBe('2026-10-30T01:02:03Z')
    expect(view.lines[0]!.level).toBe('WARN')
    expect(view.lines[0]!.target).toBe('mqtt')
    expect(view.lines[0]!.message).toBe('连接重试中')
    expect(view.lines[0]!.raw).toBe(raw)
  })

  // 边界：非 JSON 行原样保留在 message（现场日志常有半行/裸文本）
  it('viewArchive 对非 JSON 行原样保留在 message', async () => {
    requestMock.mockResolvedValue('plain text line')

    const view = await viewArchive('2026-10-30')

    expect(view.lines).toHaveLength(1)
    expect(view.lines[0]!.message).toBe('plain text line')
    expect(view.lines[0]!.raw).toBe('plain text line')
    expect(view.lines[0]!.time).toBe('')
    expect(view.lines[0]!.level).toBe('')
  })

  // 边界：空行/空白行必须被过滤，不能变成空日志行占位
  it('viewArchive 过滤空行与空白行', async () => {
    requestMock.mockResolvedValue('{"level":"info"}\n\n   \n{"level":"error"}')

    const view = await viewArchive('2026-10-30')

    expect(view.lines).toHaveLength(2)
    expect(view.lines.map((l) => l.level)).toEqual(['INFO', 'ERROR'])
  })

  // 锁定错误分支语义：取样失败**原样 reject**（与其余 api 模块一致），
  // 由页面 catch 后如实提示「该日无日志」，不用占位日志冒充现场数据。
  // 早前这里吞异常返回 { error }，调用方的 catch 会失效（W3a-2）。
  it('viewArchive 取样失败时原样 reject，不吞异常', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(404): 归档不存在'))

    await expect(viewArchive('2026-10-30')).rejects.toThrow('归档不存在')
  })

  // 边界：后端抛的不是 Error 时也应透传（页面侧负责兜底文案）
  it('viewArchive 遇到非 Error 异常时同样 reject', async () => {
    requestMock.mockRejectedValue('boom')

    await expect(viewArchive('2026-10-30')).rejects.toBe('boom')
  })

  // 锁定下载契约：行数固定 20000（后端 MAX_LINES 上限），文件名 gateway-<date>.log，
  // Blob MIME 为 text/plain;charset=utf-8。
  it('downloadArchive 走下载端点并以 gateway-<date>.log 触发保存', async () => {
    requestMock.mockResolvedValue('log content')

    await downloadArchive('2026-10-30')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/diagnostics/logs/download?lines=20000&date=2026-10-30')

    expect(triggerDownloadMock).toHaveBeenCalledTimes(1)
    const [blob, filename] = triggerDownloadMock.mock.calls[0]! as [Blob, string]
    expect(filename).toBe('gateway-2026-10-30.log')
    expect(blob.type).toBe('text/plain;charset=utf-8')
  })

  // 边界：不传日期时下载当期日志，文件名固定为 gateway-current.log
  it('downloadArchive 不传日期时用 gateway-current.log 且不带 date 参数', async () => {
    requestMock.mockResolvedValue('current log')

    await downloadArchive()

    expect(firstCallConfig().url).toBe('/api/diagnostics/logs/download?lines=20000')
    const [, filename] = triggerDownloadMock.mock.calls[0]! as [Blob, string]
    expect(filename).toBe('gateway-current.log')
  })
})
