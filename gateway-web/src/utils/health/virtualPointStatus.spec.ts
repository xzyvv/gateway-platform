import { describe, it, expect } from 'vitest'
import { virtualPointEvalStatus, virtualPointEvalUi } from './virtualPointStatus'

describe('virtualPointEvalStatus', () => {
  it('已成功求值 → ok', () => {
    expect(
      virtualPointEvalStatus({ enabled: true, lastValue: 2200, error: null }),
    ).toBe('ok')
  })

  it('引擎尚未求值 → pending（不是故障）', () => {
    expect(
      virtualPointEvalStatus({
        enabled: true,
        lastValue: null,
        error: '尚未求值（等待采集数据）',
      }),
    ).toBe('pending')
  })

  it('引用点位无新鲜值 → waiting', () => {
    expect(
      virtualPointEvalStatus({
        enabled: true,
        lastValue: null,
        error: '引用点位不可用: current',
      }),
    ).toBe('waiting')
  })

  it('公式语法错误 → error', () => {
    expect(
      virtualPointEvalStatus({
        enabled: true,
        lastValue: null,
        error: '公式语法错误: 括号不匹配',
      }),
    ).toBe('error')
  })

  it('公式求值失败（除零） → error', () => {
    expect(
      virtualPointEvalStatus({
        enabled: true,
        lastValue: null,
        error: '公式求值失败: 除数为零',
      }),
    ).toBe('error')
  })

  it('已停用 → disabled（即便有历史值）', () => {
    expect(
      virtualPointEvalStatus({ enabled: false, lastValue: 2200, error: null }),
    ).toBe('disabled')
  })

  it('无值且无错误 → pending', () => {
    expect(
      virtualPointEvalStatus({ enabled: true, lastValue: null, error: null }),
    ).toBe('pending')
  })
})

describe('virtualPointEvalUi', () => {
  it('pending 用 info 配色，不标红', () => {
    const ui = virtualPointEvalUi({
      enabled: true,
      lastValue: null,
      error: '尚未求值（等待采集数据）',
    })
    expect(ui.type).toBe('info')
    expect(ui.text).toBe('待采集')
  })

  it('error 用 danger 配色', () => {
    const ui = virtualPointEvalUi({
      enabled: true,
      lastValue: null,
      error: '公式语法错误: xxx',
    })
    expect(ui.type).toBe('danger')
    expect(ui.text).toBe('求值异常')
  })

  it('waiting 用 warning 配色', () => {
    const ui = virtualPointEvalUi({
      enabled: true,
      lastValue: null,
      error: '引用点位不可用: current',
    })
    expect(ui.type).toBe('warning')
  })
})
