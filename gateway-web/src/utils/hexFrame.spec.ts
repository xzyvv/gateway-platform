import { describe, it, expect } from 'vitest'
import { isWriteFrame } from './hexFrame'

/**
 * DD-30 / StepDebug 共用：写帧判定的口径钉子。
 * 口径与后端 packet_editor.rs::judge_modbus 的 0x05|0x06|0x0F|0x10 一致。
 * 字节 0 是从站地址、字节 1 是功能码——不要在字节 0 上误判。
 */
describe('isWriteFrame 写帧判定', () => {
  it.each([
    ['01 05 00 00 FF 00', 5, '写单线圈'],
    ['01 06 00 00 00 01', 6, '写单寄存器'],
    ['01 0F 00 00 00 08 01 FF', 15, '写多线圈'],
    ['01 10 00 00 00 02 04 00 01 00 02', 16, '写多寄存器'],
  ])('写帧 %s（FC%i - %s）判为写帧', (hex, _fc, _label) => {
    expect(isWriteFrame(hex)).toBe(true)
  })

  it.each([
    ['01 03 00 00 00 02', '读保持寄存器'],
    ['01 04 00 00 00 02', '读输入寄存器'],
    ['01 01 00 00 00 08', '读线圈'],
    ['01 02 00 00 00 08', '读离散输入'],
  ])('%s（%s）判为读帧，不触发写确认', (hex, _label) => {
    expect(isWriteFrame(hex)).toBe(false)
  })

  it('小写 + 无空格的紧凑帧同样识别', () => {
    expect(isWriteFrame('010600010001')).toBe(true)
    expect(isWriteFrame('010300010001')).toBe(false)
  })

  it('从站地址恰好是 05/06 但功能码是读（03）：不得误判为写帧', () => {
    // 回归钉子：只认字节 1 的功能码，不能被字节 0 的地址误导
    expect(isWriteFrame('05 03 00 00 00 01')).toBe(false)
    expect(isWriteFrame('06 03 00 00 00 01')).toBe(false)
  })

  it('异常帧（功能码最高位 1，如 0x83）不判为写帧', () => {
    expect(isWriteFrame('01 83 02')).toBe(false)
  })
})
