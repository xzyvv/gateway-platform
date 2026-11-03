import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 与 ElMessage 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock, elMessageMock } = vi.hoisted(() => ({ requestMock: vi.fn(), elMessageMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))
// config.ts 以函数形式调用 ElMessage({ type, message, duration, showClose })
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: elMessageMock }
})

import {
  backupConfig,
  compareConfigVersions,
  configDiff,
  exportConfig,
  getConfig,
  getConfigVersion,
  importConfig,
  listConfigVersions,
  postConfig,
  reloadConfig,
  rollbackConfigVersion,
  validateConfigDraft,
} from './config'
import type { GatewayConfig } from './types'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
  params?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

/** 最小可用配置对象（只为验证请求体形状，字段用断言补齐） */
function sampleConfig(): GatewayConfig {
  return { schema_version: 1 } as unknown as GatewayConfig
}

describe('config 配置读写契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
    elMessageMock.mockReset()
  })

  // 锁定读路径：GET /api/config（token 由后端脱敏），不附加任何查询参数
  it('getConfig 走 GET /api/config 且不带查询参数', async () => {
    const cfg = sampleConfig()
    requestMock.mockResolvedValue(cfg)

    await expect(getConfig()).resolves.toBe(cfg)

    const call = firstCallConfig()
    expect(call.method).toBe('GET')
    expect(call.url).toBe('/api/config')
    expect(call.params).toBeUndefined()
  })

  // 锁定保存路径与请求体形状：后端 post_config 直接吃 Json<GatewayConfig>，
  // 所以 data 必须是**原始 config**（不能包成 { config }），否则后端反序列化必失败。
  it('postConfig 走 POST /api/config，请求体是原始 config（不包裹）', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })
    const cfg = sampleConfig()

    await postConfig(cfg)

    const call = firstCallConfig()
    expect(call.method).toBe('POST')
    expect(call.url).toBe('/api/config')
    expect(call.data).toBe(cfg)
    expect((call.data as { config?: unknown }).config).toBeUndefined()
  })

  // 锁定 restart_required 提示：后端返回配置节 key，需映射成中文展示名并以 warning 提示，
  // 否则用户以为「保存成功但行为没变」而反复保存。
  it('postConfig 在 restart_required 非空时弹出 warning 提示（分节映射为展示名）', async () => {
    requestMock.mockResolvedValue({ status: 'ok', restart_required: ['webhook', 'opcua'] })

    await postConfig(sampleConfig())

    expect(elMessageMock).toHaveBeenCalledTimes(1)
    const arg = elMessageMock.mock.calls[0]![0] as {
      type: string
      message: string
      duration: number
      showClose: boolean
    }
    expect(arg.type).toBe('warning')
    expect(arg.message).toContain('Webhook')
    expect(arg.message).toContain('OPC UA 服务端')
    expect(arg.duration).toBe(8000)
    expect(arg.showClose).toBe(true)
  })

  // 未知分节（后端新增了前端没配标签的 key）必须回退显示原 key，不能渲染成 undefined
  it('postConfig 对未知分节回退显示原始 key', async () => {
    requestMock.mockResolvedValue({ status: 'ok', restart_required: ['brand_new_section'] })

    await postConfig(sampleConfig())

    const arg = elMessageMock.mock.calls[0]![0] as { message: string }
    expect(arg.message).toContain('brand_new_section')
    expect(arg.message).not.toContain('undefined')
  })

  // 锁定 notes 提示：后端 notes 是「热重载有分节未成功」的证据（如 MQTT 重连失败），
  // 必须以 error 提示暴露给用户，不可静默丢弃。
  it('postConfig 在 notes 非空时弹出 error 提示并给出条数', async () => {
    requestMock.mockResolvedValue({
      status: 'ok',
      notes: ['mqtt reload failed (kept old): new mqtt client failed to connect within 10s'],
    })

    await postConfig(sampleConfig())

    expect(elMessageMock).toHaveBeenCalledTimes(1)
    const arg = elMessageMock.mock.calls[0]![0] as { type: string; message: string; duration: number }
    expect(arg.type).toBe('error')
    expect(arg.message).toContain('1 项未生效')
    expect(arg.message).toContain('mqtt reload failed')
    expect(arg.duration).toBe(10000)
  })

  // 旧版后端不返回 restart_required / notes：此时必须静默（不弹任何提示）
  it('postConfig 在无 restart_required / notes 时不弹提示（兼容旧后端）', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })

    await postConfig(sampleConfig())

    expect(elMessageMock).not.toHaveBeenCalled()
  })

  // 锁定校验请求体：后端 ConfigValidateRequest 是 { config } 包裹体，
  // 与 postConfig 的原始体不同——两者不能被"统一风格"改错。
  it('validateConfigDraft 走 POST /api/config/validate，请求体是 { config } 包裹体', async () => {
    requestMock.mockResolvedValue({ valid: true, errors: 0, warnings: 0, issues: [] })
    const cfg = sampleConfig()

    await validateConfigDraft(cfg)

    const call = firstCallConfig()
    expect(call.method).toBe('POST')
    expect(call.url).toBe('/api/config/validate')
    expect(call.data).toEqual({ config: cfg })
  })

  // 锁定导出语义：默认脱敏（不带 include_secrets），true 时才带。
  // 注意 params 缺省是 undefined 而不是 {}（避免线上多一个空对象查询参数）。
  it('exportConfig 默认不传 include_secrets，includeSecrets=true 时才传', async () => {
    requestMock.mockResolvedValue({ schema_version: 1 })

    await exportConfig()
    expect(firstCallConfig().params).toBeUndefined()

    requestMock.mockClear()
    await exportConfig(true)
    expect(firstCallConfig().params).toEqual({ include_secrets: true })
  })

  // 锁定导入：PUT /api/config/import + { config } 包裹体
  it('importConfig 走 PUT /api/config/import，请求体是 { config } 包裹体', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })
    const cfg = sampleConfig()

    await importConfig(cfg)

    const call = firstCallConfig()
    expect(call.method).toBe('PUT')
    expect(call.url).toBe('/api/config/import')
    expect(call.data).toEqual({ config: cfg })
  })

  // 锁定热重载路径：POST /api/config/reload（不带请求体）
  it('reloadConfig 走 POST /api/config/reload 且不带请求体', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })

    await reloadConfig()

    const call = firstCallConfig()
    expect(call.method).toBe('POST')
    expect(call.url).toBe('/api/config/reload')
    expect(call.data).toBeUndefined()
  })

  // 锁定版本列表：后端返回 { versions: [...] } 包裹体，前端必须解开成数组
  it('listConfigVersions 走 GET /api/config/versions 并把 { versions } 解开为数组', async () => {
    const versions = [{ id: 3, created_at: 1700000000, message: 'backup', size: 12, is_current: true }]
    requestMock.mockResolvedValue({ versions })

    await expect(listConfigVersions()).resolves.toEqual(versions)

    const call = firstCallConfig()
    expect(call.method).toBe('GET')
    expect(call.url).toBe('/api/config/versions')
  })

  // 边界：后端返回体缺 versions 字段时回退空数组，避免页面拿到 undefined 而崩溃
  it('listConfigVersions 在响应缺少 versions 时回退空数组', async () => {
    requestMock.mockResolvedValue({})
    await expect(listConfigVersions()).resolves.toEqual([])
  })

  // 锁定单版本快照路径：GET /api/config/versions/:id
  it('getConfigVersion 走 GET /api/config/versions/:id', async () => {
    const snap = { schema_version: 1 }
    requestMock.mockResolvedValue(snap)

    await expect(getConfigVersion(7)).resolves.toBe(snap)

    const call = firstCallConfig()
    expect(call.method).toBe('GET')
    expect(call.url).toBe('/api/config/versions/7')
  })

  // 锁定回滚路径：POST /api/config/versions/:id/rollback
  it('rollbackConfigVersion 走 POST /api/config/versions/:id/rollback', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })

    await rollbackConfigVersion(7)

    const call = firstCallConfig()
    expect(call.method).toBe('POST')
    expect(call.url).toBe('/api/config/versions/7/rollback')
  })

  // 锁定 diff 请求体：后端 config_diff 直接吃 Json<GatewayConfig>，与 postConfig 同样是原始体
  it('configDiff 走 POST /api/config/diff，请求体是原始 config', async () => {
    requestMock.mockResolvedValue({ changed_fields: ['mqtt'], has_changes: true })
    const cfg = sampleConfig()

    await configDiff(cfg)

    const call = firstCallConfig()
    expect(call.method).toBe('POST')
    expect(call.url).toBe('/api/config/diff')
    expect(call.data).toBe(cfg)
  })

  // 锁定手动备份路径：POST /api/config/backup
  it('backupConfig 走 POST /api/config/backup', async () => {
    requestMock.mockResolvedValue({ status: 'ok', version_id: 5, points: 12, devices: 3 })

    await backupConfig()

    const call = firstCallConfig()
    expect(call.method).toBe('POST')
    expect(call.url).toBe('/api/config/backup')
  })

  // 锁定版本对比路径：compare_to 以查询串直接拼进 url（不是 axios params），
  // 误改成 params 会让后端拿不到 compare_to、退化成单版本自比较。
  it('compareConfigVersions 把 compare_to 拼进 url 查询串', async () => {
    requestMock.mockResolvedValue({ changed_fields: [], has_changes: false })

    await compareConfigVersions(7, 9)

    const call = firstCallConfig()
    expect(call.method).toBe('GET')
    expect(call.url).toBe('/api/config/versions/7/compare?compare_to=9')
    expect(call.params).toBeUndefined()
  })

  // 错误分支：后端失败原样 reject，由页面给出「保存失败」等具体文案，api 层不吞错
  it('后端报错时 postConfig 原样 reject', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(400): 配置校验失败'))
    await expect(postConfig(sampleConfig())).rejects.toThrow('配置校验失败')
  })
})
