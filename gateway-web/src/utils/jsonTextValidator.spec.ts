/**
 * JSON 文本校验的统一契约（R1）。
 *
 * 三条导入路径共用这一份解析与中文提示；这里锁住「失败不抛异常」与
 * 「提示必须带字段名」，否则现场只能看到一条没有定位信息的报错。
 */
import { describe, it, expect } from 'vitest'
import { makeJsonTextValidator, parseJsonText } from './jsonTextValidator'

describe('parseJsonText', () => {
  it('空文本给出带字段名的「不能为空」提示', () => {
    const r = parseJsonText('   ', '待导入的配置')
    expect(r.ok).toBe(false)
    expect(r.error).toBe('待导入的配置不能为空')
  })

  it('合法 JSON 返回解析后的值', () => {
    const r = parseJsonText<{ a: number }>('{"a":1}', '配置')
    expect(r.ok).toBe(true)
    expect(r.value).toEqual({ a: 1 })
  })

  it('非法 JSON 不抛异常，返回带字段名的中文原因', () => {
    const r = parseJsonText('{bad', '工程文件')
    expect(r.ok).toBe(false)
    expect(r.error).toMatch(/^工程文件格式错误，无法解析：/)
  })
})

describe('makeJsonTextValidator', () => {
  it('通过时调用 callback 且不传错误', () => {
    const validator = makeJsonTextValidator('配置')!
    const calls: Array<Error | undefined> = []
    // Element Plus 的 validator 签名包含 rule/source/options，这里只关心 callback 结果
    ;(validator as unknown as (_r: unknown, v: unknown, cb: (e?: Error) => void) => void)(
      undefined,
      '{"ok":true}',
      (e) => calls.push(e),
    )
    expect(calls).toEqual([undefined])
  })

  it('解析失败时把原因交给表单，显示在输入框下方', () => {
    const validator = makeJsonTextValidator('片段')!
    const calls: Array<Error | undefined> = []
    ;(validator as unknown as (_r: unknown, v: unknown, cb: (e?: Error) => void) => void)(
      undefined,
      '{oops',
      (e) => calls.push(e),
    )
    expect(calls).toHaveLength(1)
    expect(calls[0]).toBeInstanceOf(Error)
    expect(calls[0]!.message).toMatch(/^片段格式错误，无法解析：/)
  })
})
