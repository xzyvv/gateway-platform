import { describe, it, expect } from 'vitest'
import { ERR, ERROR_CODES, describeErrorCode, isKnownErrorCode, describeErrorDetail, isHttpStatusCode } from './errorCodes'

/**
 * 错误码字典的取值优先级：后端 message > 字典兜底 > 裸码。
 * 这里钉住优先级，避免以后有人改成"字典优先"而覆盖掉后端更精确的文案。
 */
describe('describeErrorCode', () => {
  it('后端 message 非空时优先原样透出', () => {
    expect(describeErrorCode(ERR.QUOTA_EXCEEDED, '当前档位支持最多 1500 采集点数')).toBe(
      '当前档位支持最多 1500 采集点数',
    )
  })

  it('message 只含空白时视为缺失，回落到字典', () => {
    expect(describeErrorCode(ERR.UNAUTHORIZED, '   ')).toBe('登录状态已失效，请重新登录')
  })

  it('已知码且无 message：字典文案 + 处置建议', () => {
    expect(describeErrorCode(ERR.QUOTA_EXCEEDED)).toBe('当前授权配额不足，请联系供应商升级授权')
    expect(describeErrorCode(ERR.UNAUTHORIZED)).toBe('登录状态已失效，请重新登录')
  })

  it('未知码且无 message：退化为「错误码 N」，不抛错', () => {
    expect(describeErrorCode(7777)).toBe('错误码 7777')
    expect(isKnownErrorCode(7777)).toBe(false)
  })

  it('后端英文 message：翻译成中文，不把开发者文案丢给现场', () => {
    expect(describeErrorCode(ERR.GROUP_NOT_FOUND, 'group not found')).toBe('设备分组不存在，可能已被删除')
    expect(describeErrorCode(ERR.LICENSE_REQUIRED, 'license required')).toContain('授权')
    // 未收录的后端中文文案照旧原样优先
    expect(describeErrorCode(ERR.INVALID_PARAM, '采集周期需在 1~59 之间')).toBe('采集周期需在 1~59 之间')
  })

  it('授权类三码语义分离：未导入 / 已失效 / 配额不足', () => {
    // P6：后端按 license 状态分码，三者必须给出不同处置指引
    expect(describeErrorCode(ERR.LICENSE_REQUIRED)).toContain('导入')
    expect(describeErrorCode(ERR.LICENSE_INVALID)).toContain('失效')
    expect(describeErrorCode(ERR.QUOTA_EXCEEDED)).toContain('配额不足')
    // 后端 7003 的英文短句要能翻译成中文
    expect(describeErrorCode(ERR.LICENSE_INVALID, 'license expired or invalid')).toContain('授权')
  })

  it('码表常量与拦截器约定一致（8000 鉴权 / 4021 配额）', () => {
    expect(ERR.OK).toBe(0)
    expect(ERR.UNAUTHORIZED).toBe(8000)
    expect(ERR.QUOTA_EXCEEDED).toBe(4021)
    // 数字键按升序枚举
    const codes = Object.keys(ERROR_CODES).map(Number)
    expect(codes).toEqual([...codes].sort((a, b) => a - b))
  })

  it('v2.0 已删除档位/feature 中间件码（7002 / 7004）', () => {
    // 后端 license v2.0 只保留配额约束，7002/7004 语义已废弃且永不下发；
    // 前端同步删除，避免残留码表误导读者以为还有档位门控。
    expect(isKnownErrorCode(7002)).toBe(false)
    expect(isKnownErrorCode(7004)).toBe(false)
  })
})

/**
 * 通知中心的错误码详情映射：HTTP 状态码给「是什么 + 下一步」，
 * 已知业务码复用字典文案；未知码回退裸码展示。
 */
describe('describeErrorDetail', () => {
  it('HTTP 状态码区间判定：100–599 命中，业务码在区间外', () => {
    expect(isHttpStatusCode(502)).toBe(true)
    expect(isHttpStatusCode(100)).toBe(true)
    expect(isHttpStatusCode(599)).toBe(true)
    // 业务码 4021/8000 不得误判为 HTTP 状态码
    expect(isHttpStatusCode(ERR.QUOTA_EXCEEDED)).toBe(false)
    expect(isHttpStatusCode(ERR.UNAUTHORIZED)).toBe(false)
    expect(isHttpStatusCode(0)).toBe(false)
    expect(isHttpStatusCode(99)).toBe(false)
    expect(isHttpStatusCode(600)).toBe(false)
    expect(isHttpStatusCode(1.5)).toBe(false)
  })

  it('HTTP 状态码：返回人话解释与排查建议', () => {
    expect(describeErrorDetail(502)).toEqual({
      label: '网关服务未就绪',
      suggestion: '网关可能正在重启，请等待几秒后刷新页面',
    })
    expect(describeErrorDetail(500)!.suggestion).toContain('系统日志')
    expect(describeErrorDetail(400)!.label).toBe('请求参数有误')
  })

  it('已知业务码：复用字典 message 与 hint', () => {
    expect(describeErrorDetail(ERR.UNAUTHORIZED)).toEqual({
      label: '登录状态已失效',
      suggestion: '请重新登录',
    })
    expect(describeErrorDetail(ERR.QUOTA_EXCEEDED)!.label).toBe('当前授权配额不足')
  })

  it('未知业务码返回 undefined（调用方回退「错误码 N」）', () => {
    expect(describeErrorDetail(7777)).toBeUndefined()
  })
})
