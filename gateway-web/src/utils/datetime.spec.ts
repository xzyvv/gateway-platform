import { describe, it, expect } from 'vitest'
import { formatClock, formatDate, formatDateTime, formatTimeOfDay, pad2 } from './datetime'

// 用「本地时刻」构造，避免用例本身依赖运行机器时区
const sample = new Date(2026, 9, 24, 12, 31, 14) // 2026-10-24 12:31:14 本地时间

describe('datetime', () => {
  it('formatDateTime 统一 24 小时制', () => {
    expect(formatDateTime(sample)).toBe('2026/10/24 12:31:14')
  })

  // 24 小时制是硬约定：中文环境默认 12 小时制会出「下午12:31:14」，同屏无法横向比对
  it('formatTimeOfDay 不带上午/下午', () => {
    const text = formatTimeOfDay(sample)
    expect(text).toBe('12:31:14')
    expect(text).not.toMatch(/上午|下午|AM|PM/)
  })

  it('formatDate 只到日期', () => {
    expect(formatDate(sample)).toBe('2026/10/24')
  })

  it('formatClock 等价于当前时刻的 formatTimeOfDay', () => {
    expect(formatClock()).toMatch(/^\d{2}:\d{2}:\d{2}$/)
  })

  // 无效输入一律空串：不能让 "Invalid Date" 漏进界面或导出文件
  it('无效输入返回空串', () => {
    for (const bad of [null, undefined, '', 'abc', NaN, new Date('x')]) {
      expect(formatDateTime(bad)).toBe('')
      expect(formatDate(bad)).toBe('')
      expect(formatTimeOfDay(bad)).toBe('')
    }
  })

  // 时间戳与字符串输入都要能用（后端有的给 ms 数字，有的给 ISO 串）
  it('接受时间戳与 ISO 字符串', () => {
    expect(formatTimeOfDay(sample.getTime())).toBe('12:31:14')
    expect(formatTimeOfDay(sample.toISOString())).toBe('12:31:14')
  })

  it('pad2 补到两位，三位以上原样', () => {
    expect(pad2(0)).toBe('00')
    expect(pad2(7)).toBe('07')
    expect(pad2(42)).toBe('42')
    expect(pad2(123)).toBe('123')
  })
})
