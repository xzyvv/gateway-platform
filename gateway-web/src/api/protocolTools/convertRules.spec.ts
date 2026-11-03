import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
// protocolTools 下的模块，request 来自上一层 barrel → mock 路径是 '../index'
vi.mock('../index', () => ({ request: requestMock }))

import {
  listConvertRules,
  createConvertRule,
  updateConvertRule,
  deleteConvertRule,
  toggleConvertRule,
} from './convertRules'
import { toConvertRule } from './types'
import type { ConvertRule as ConvertRuleRaw } from '../generated'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function ruleRow(patch: Partial<ConvertRuleRaw> = {}): ConvertRuleRaw {
  return {
    id: 'cr-1',
    name: '压力→温度',
    source_proto: 'modbus',
    source_point_id: 'p1',
    source_point_name: 'PT01',
    target_proto: 'mqtt',
    target_tag: 'temp',
    transform: 'x * 0.1',
    scale: 0.1,
    offset: null,
    enum_map: null,
    enabled: true,
    last_converted_at_ms: 1700000000000,
    last_value: '21.5',
    created_at_ms: 1699999999000,
    ...patch,
  }
}

describe('convertRules 转换规则 CRUD 契约', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定列表契约：GET /api/protocol-tools/convert/rules
  it('列出规则走 GET /api/protocol-tools/convert/rules', async () => {
    requestMock.mockResolvedValue([])

    await listConvertRules()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/protocol-tools/convert/rules')
  })

  // 锁定列表映射契约：snake_case → camelCase，bigint 时间戳 → number
  it('规则列表转换为 camelCase 视图', async () => {
    requestMock.mockResolvedValue([ruleRow()])

    const rows = await listConvertRules()

    expect(rows[0]).toEqual({
      id: 'cr-1',
      name: '压力→温度',
      sourceProto: 'modbus',
      sourcePointId: 'p1',
      sourcePointName: 'PT01',
      targetProto: 'mqtt',
      targetTag: 'temp',
      transform: 'x * 0.1',
      scale: 0.1,
      offset: null,
      enumMap: null,
      enabled: true,
      lastConvertedAt: 1700000000000,
      lastValue: '21.5',
      createdAt: 1699999999000,
    })
  })

  // 锁定创建契约：POST 同路径，请求体是 snake_case 且 id 传 ''、created_at_ms 传 0 交由后端补全
  it('创建规则走 POST /api/protocol-tools/convert/rules，请求体为 snake_case 且 id/created_at_ms 交后端补全', async () => {
    requestMock.mockResolvedValue(ruleRow())

    await createConvertRule({
      name: '压力→温度',
      sourceProto: 'modbus',
      sourcePointId: 'p1',
      sourcePointName: 'PT01',
      targetProto: 'mqtt',
      targetTag: 'temp',
      transform: 'x * 0.1',
      enabled: true,
    })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/protocol-tools/convert/rules')
    expect(cfg.data).toEqual({
      id: '',
      name: '压力→温度',
      source_proto: 'modbus',
      source_point_id: 'p1',
      source_point_name: 'PT01',
      target_proto: 'mqtt',
      target_tag: 'temp',
      transform: 'x * 0.1',
      scale: null,
      offset: null,
      enum_map: null,
      enabled: true,
      last_converted_at_ms: null,
      last_value: null,
      created_at_ms: 0,
    })
  })

  // 锁定更新契约：PUT /api/protocol-tools/convert/rules/:id，请求体 id 与路径 id 一致
  it('更新规则走 PUT /api/protocol-tools/convert/rules/:id，请求体 id 与路径一致', async () => {
    requestMock.mockResolvedValue(undefined)

    await updateConvertRule('cr-1', { name: '新名', enabled: false })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe('/api/protocol-tools/convert/rules/cr-1')
    expect((cfg.data as Record<string, unknown>).id).toBe('cr-1')
    expect((cfg.data as Record<string, unknown>).name).toBe('新名')
    expect((cfg.data as Record<string, unknown>).enabled).toBe(false)
  })

  // 锁定更新缺省契约：未提供的字段补齐默认值（空串 / null / false），保证后端反序列化不缺字段
  it('更新规则时缺失字段补默认值（不会发送 undefined）', async () => {
    requestMock.mockResolvedValue(undefined)

    await updateConvertRule('cr-1', { name: '只有名字' })

    expect(firstCallConfig().data).toEqual({
      id: 'cr-1',
      name: '只有名字',
      source_proto: '',
      source_point_id: '',
      source_point_name: '',
      target_proto: '',
      target_tag: '',
      transform: '',
      scale: null,
      offset: null,
      enum_map: null,
      enabled: false,
      last_converted_at_ms: null,
      last_value: null,
      created_at_ms: 0,
    })
  })

  // 锁定 id 编码契约：规则 id 做 URL 编码
  it('规则 id 做 URL 编码', async () => {
    requestMock.mockResolvedValue(undefined)

    await deleteConvertRule('a/b')

    expect(firstCallConfig().url).toBe('/api/protocol-tools/convert/rules/a%2Fb')
  })

  // 锁定删除契约：DELETE /api/protocol-tools/convert/rules/:id
  it('删除规则走 DELETE /api/protocol-tools/convert/rules/:id', async () => {
    requestMock.mockResolvedValue(undefined)

    await deleteConvertRule('cr-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/protocol-tools/convert/rules/cr-1')
  })

  // 锁定启停契约：POST /api/protocol-tools/convert/rules/:id/toggle（子路径，不是 PUT）
  it('启停规则走 POST /api/protocol-tools/convert/rules/:id/toggle，返回布尔值', async () => {
    requestMock.mockResolvedValue(true)

    const res = await toggleConvertRule('cr-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/protocol-tools/convert/rules/cr-1/toggle')
    expect(res).toBe(true)
  })
})

describe('protocolTools toConvertRule 转换契约', () => {
  // 锁定 lastConvertedAt 边界：后端从未转换过时为 null，视图层保持 null（不是 0）
  it('toConvertRule：last_converted_at_ms 为 null 时 lastConvertedAt 为 null', () => {
    const v = toConvertRule(ruleRow({ last_converted_at_ms: null as unknown as number }))

    expect(v.lastConvertedAt).toBeNull()
  })

  // 锁定 enumMap 转换：数组原样透传，元素保持 key/label
  it('toConvertRule：enum_map 原样透传', () => {
    const v = toConvertRule(ruleRow({ enum_map: [{ key: '0', label: '关' }] }))

    expect(v.enumMap).toEqual([{ key: '0', label: '关' }])
  })
})
