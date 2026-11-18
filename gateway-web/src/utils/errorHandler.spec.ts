import { describe, it, expect } from 'vitest'
import {
  composeFailure,
  extractErrorMessage,
  extractErrorReason,
  failureMessage,
  isUserNotified,
  markUserNotified,
} from './errorHandler'

describe('composeFailure', () => {
  it('拼「动作：原因」', () => {
    expect(composeFailure('保存点位失败', '点位名称不能为空')).toBe('保存点位失败：点位名称不能为空')
  })

  // 原因缺失时不能留一个「保存失败：」的半截文案
  it('原因为空只回动作', () => {
    expect(composeFailure('保存点位失败', '')).toBe('保存点位失败')
    expect(composeFailure('保存点位失败', '   ')).toBe('保存点位失败')
  })

  // 调用方没给语境时不该凭空造「操作失败」（旧实现会）
  it('动作为空只回原因', () => {
    expect(composeFailure(undefined, '总线不存在')).toBe('总线不存在')
    expect(composeFailure('', '总线不存在')).toBe('总线不存在')
  })

  it('原因与动作相同不重复', () => {
    expect(composeFailure('保存失败', '保存失败')).toBe('保存失败')
  })

  // 后端把动作写进 message 的情况（如「导出配置失败，请稍后重试」）不能变成双重前缀
  it('原因已以动作开头时只回原因', () => {
    expect(composeFailure('导出配置失败', '导出配置失败，请稍后重试')).toBe('导出配置失败，请稍后重试')
  })

  it('两侧空白先裁掉，不产生「动作 ：原因」', () => {
    expect(composeFailure('  保存失败 ', ' 库锁 ')).toBe('保存失败：库锁')
  })
})

describe('failureMessage', () => {
  // 与 composeFailure 同一拼装口径，只是入参的 action 不含「失败」
  it('给不含「失败」的动词补上「失败：原因」', () => {
    expect(failureMessage('保存点位', new Error('点位名称不能为空'))).toBe(
      '保存点位失败：点位名称不能为空',
    )
  })

  it('原因缺失退回「请稍后重试」，不留半截文案', () => {
    expect(failureMessage('保存点位', null)).toBe('保存点位失败，请稍后重试')
    expect(failureMessage('保存点位', new Error(''))).toBe('保存点位失败，请稍后重试')
  })
})

describe('用户提示标记（避免全局 + 页面双弹）', () => {
  it('默认未标记', () => {
    expect(isUserNotified(new Error('boom'))).toBe(false)
  })

  it('标记后可识别，且不改动 Error 的可枚举属性', () => {
    const err = new Error('boom')
    markUserNotified(err)
    expect(isUserNotified(err)).toBe(true)
    // 日志/遥测序列化 Error 时不会把内部标记带出去
    expect(Object.keys(err)).toHaveLength(0)
    expect(JSON.stringify(err)).not.toContain('userNotified')
  })

  it('多个错误对象互不影响', () => {
    const a = new Error('a')
    const b = new Error('b')
    markUserNotified(a)
    expect(isUserNotified(a)).toBe(true)
    expect(isUserNotified(b)).toBe(false)
  })

  it('非对象入参不抛错', () => {
    expect(() => markUserNotified('boom')).not.toThrow()
    expect(() => markUserNotified(null)).not.toThrow()
    expect(isUserNotified('boom')).toBe(false)
    expect(isUserNotified(undefined)).toBe(false)
  })

  it('冻结对象打标记失败时退化为「未提示」而不是抛错', () => {
    const frozen = Object.freeze(new Error('frozen'))
    expect(() => markUserNotified(frozen)).not.toThrow()
    expect(isUserNotified(frozen)).toBe(false)
  })
})

describe('extractErrorMessage', () => {
  it('Error 取 message，字符串原样，其它用兜底', () => {
    expect(extractErrorMessage(new Error('x'))).toBe('x')
    expect(extractErrorMessage('y')).toBe('y')
    expect(extractErrorMessage(null, '兜底')).toBe('兜底')
  })
})

describe('extractErrorReason', () => {
  it('Error 取 message；message 为空则回落兜底', () => {
    expect(extractErrorReason(new Error('磁盘已满'), '未知错误')).toBe('磁盘已满')
    expect(extractErrorReason(new Error(''), '未知错误')).toBe('未知错误')
  })

  it('字符串原样返回；空串回落兜底', () => {
    expect(extractErrorReason('连接超时', '未知错误')).toBe('连接超时')
    expect(extractErrorReason('', '未知错误')).toBe('未知错误')
  })

  it('null / undefined / 0 / false / NaN 都算「没有原因」', () => {
    expect(extractErrorReason(null, '未知错误')).toBe('未知错误')
    expect(extractErrorReason(undefined, '未知错误')).toBe('未知错误')
    expect(extractErrorReason(0, '未知错误')).toBe('未知错误')
    expect(extractErrorReason(false, '未知错误')).toBe('未知错误')
    expect(extractErrorReason(Number.NaN, '未知错误')).toBe('未知错误')
  })

  it('非空对象转字符串，不吞掉信息', () => {
    expect(extractErrorReason({ code: 500 }, '未知错误')).toBe('[object Object]')
  })
})
