import { describe, it, expect } from 'vitest'
import { statusTagType, type TagType } from './statusType'

/**
 * 全站唯一状态取色表的契约：
 * 同一状态词在任何页面都必须落到同一档颜色（success / warning / danger / info），
 * 这是「同词同色」的基础，改动本表必须同步更新本用例。
 */
describe('statusTagType — 全站唯一状态取色表', () => {
  const cases: Array<[string, TagType]> = [
    // 成功 / 在线
    ['正常', 'success'], ['在线', 'success'], ['成功', 'success'], ['运行中', 'success'],
    ['同步', 'success'], ['良好', 'success'], ['就绪', 'success'],
    ['success', 'success'], ['ok', 'success'], ['good', 'success'], ['online', 'success'],
    ['running', 'success'], ['synced', 'success'], ['healthy', 'success'], ['enabled', 'success'],

    // 告警 / 降级
    ['告警', 'warning'], ['警告', 'warning'], ['部分', 'warning'], ['过期', 'warning'],
    ['不确定', 'warning'], ['降级', 'warning'],
    ['warning', 'warning'], ['warn', 'warning'], ['stale', 'warning'], ['degraded', 'warning'],
    ['uncertain', 'warning'], ['pending', 'warning'],

    // 失败 / 离线
    ['失败', 'danger'], ['离线', 'danger'], ['错误', 'danger'], ['异常', 'danger'], ['超时', 'danger'],
    ['error', 'danger'], ['failed', 'danger'], ['fail', 'danger'], ['bad', 'danger'],
    ['offline', 'danger'], ['down', 'danger'], ['timeout', 'danger'], ['disconnected', 'danger'],

    // 未知 / 未启用
    ['未知', 'info'], ['未配置', 'info'], ['停用', 'info'], ['空闲', 'info'],
    ['unknown', 'info'], ['none', 'info'], ['idle', 'info'], ['stopped', 'info'],
    ['not_configured', 'info'],
  ]

  it.each(cases)('精确词 %s → %s', (status, expected) => {
    expect(statusTagType(status)).toBe(expected)
  })

  it('大小写不敏感', () => {
    expect(statusTagType('SUCCESS')).toBe('success')
    expect(statusTagType('OK')).toBe('success')
    expect(statusTagType('Failed')).toBe('danger')
    expect(statusTagType('OFFLINE')).toBe('danger')
    expect(statusTagType('Unknown')).toBe('info')
  })

  it('空值与空白串按「未知」处理', () => {
    expect(statusTagType(null)).toBe('info')
    expect(statusTagType(undefined)).toBe('info')
    expect(statusTagType('')).toBe('info')
    expect(statusTagType('   ')).toBe('info')
  })

  it('前后空格不影响取值', () => {
    expect(statusTagType(' ok ')).toBe('success')
    expect(statusTagType(' offline ')).toBe('danger')
  })

  it('未登记的新词落到 info（不猜颜色）', () => {
    expect(statusTagType('someBrandNewState')).toBe('info')
  })
})
