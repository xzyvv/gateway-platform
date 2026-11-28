import { describe, it, expect } from 'vitest'
import { hexToBytes, normalizeHex, isValidHex, isWriteFrame, byteToAscii } from './hexCodec'
// 三个历史入口：对外符号必须继续可用且与单一真源等价（DD-47 回归钉子）
import { hexToBytes as hexToBytesFromDump, byteToAscii as byteToAsciiFromDump } from './hexDump'
import { normalizeHex as normalizeHexFromFrame, isValidHex as isValidHexFromFrame, isWriteFrame as isWriteFrameFromFrame } from './hexFrame'

describe('hexCodec：HEX 字节解析单一真源（DD-47）', () => {
  describe('hexToBytes', () => {
    it('空格分隔 / 大小写混合 / 无分隔都能解析', () => {
      expect(hexToBytes('01 03 0a ff')).toEqual([1, 3, 10, 255])
      expect(hexToBytes('0103 0AFF')).toEqual([1, 3, 10, 255])
      expect(hexToBytes('99060504030201')).toEqual([0x99, 6, 5, 4, 3, 2, 1])
    })

    it('容忍 0x 前缀与任意空白（含换行/制表符）', () => {
      expect(hexToBytes('0x01 0x03')).toEqual([1, 3])
      expect(hexToBytes('  01  03\n00 ')).toEqual([1, 3, 0])
    })

    it('非法输入（空 / 奇数长度 / 非 hex 字符）一律返回空数组，不抛错也不产生 NaN', () => {
      expect(hexToBytes('')).toEqual([])
      expect(hexToBytes('01 0')).toEqual([])
      expect(hexToBytes('01 GG')).toEqual([])
      expect(hexToBytes('0x')).toEqual([])
      // 关键回归：旧 frameParser 实现对非 hex 会产生 NaN 并进入解析结果
      expect(hexToBytes('01 ZZ 03').some(Number.isNaN)).toBe(false)
      expect(hexToBytes('01 ZZ 03')).toEqual([])
    })
  })

  describe('normalizeHex / isValidHex', () => {
    it('normalizeHex 压空白并转大写', () => {
      expect(normalizeHex('  01  03\n0a ')).toBe('01 03 0A')
    })

    it('isValidHex 拒绝空串 / 奇数 / 非 hex，接受合法帧', () => {
      expect(isValidHex('')).toBe(false)
      expect(isValidHex('01 3')).toBe(false)
      expect(isValidHex('01 ZZ')).toBe(false)
      expect(isValidHex('01 03')).toBe(true)
      expect(isValidHex('0103')).toBe(true)
    })
  })

  describe('isWriteFrame（DD-30 口径）', () => {
    it('FC05/06/15/16 判为写帧，读帧不误判', () => {
      expect(isWriteFrame('01 06 00 00 00 01')).toBe(true)
      expect(isWriteFrame('01 10 00 00 00 01')).toBe(true)
      expect(isWriteFrame('01 03 00 00 00 01')).toBe(false)
      // 从站地址恰好是 05，但功能码是 03（读）—— 不得误判
      expect(isWriteFrame('05 03 00 00 00 01')).toBe(false)
    })
  })

  describe('byteToAscii', () => {
    it('可打印字符原样，控制字符与非 ASCII 用点', () => {
      expect(byteToAscii(0x41)).toBe('A')
      expect(byteToAscii(0x20)).toBe(' ')
      expect(byteToAscii(0x7e)).toBe('~')
      expect(byteToAscii(0x00)).toBe('.')
      expect(byteToAscii(0xff)).toBe('.')
    })
  })

  describe('旧入口与单一真源等价（口径不分裂）', () => {
    it('hexDump 的导出与 hexCodec 同符号一致', () => {
      expect(hexToBytesFromDump).toBe(hexToBytes)
      expect(byteToAsciiFromDump).toBe(byteToAscii)
    })

    it('hexFrame 的导出与 hexCodec 同符号一致', () => {
      expect(normalizeHexFromFrame).toBe(normalizeHex)
      expect(isValidHexFromFrame).toBe(isValidHex)
      expect(isWriteFrameFromFrame).toBe(isWriteFrame)
    })

    it('同一帧经三条历史入口得到完全相同的字节', () => {
      const frame = '68 0a 00 00 00 00 00 68 11 04 33 33 33 33'
      expect(hexToBytesFromDump(frame)).toEqual(hexToBytes(frame))
    })
  })
})
