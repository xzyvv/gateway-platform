import { describe, it, expect } from 'vitest'
import { parsePrometheusLine, parsePrometheusLines } from './prometheus'

/** 公共 Prometheus 文本分词器单测（#5：status.ts 与 MetricsDashboard 共享底层） */
describe('parsePrometheusLine', () => {
  it('解析无标签样本', () => {
    expect(parsePrometheusLine('gateway_queue_length 1234')).toEqual({
      kind: 'sample',
      sample: { name: 'gateway_queue_length', labels: {}, value: 1234 },
    })
  })

  it('解析带标签样本（含多标签与标签值中的空格）', () => {
    const r = parsePrometheusLine('http_requests_total{method="POST",url="/a b"} 5')
    expect(r).toEqual({
      kind: 'sample',
      sample: {
        name: 'http_requests_total',
        labels: { method: 'POST', url: '/a b' },
        value: 5,
      },
    })
  })

  it('解析带时间戳的样本', () => {
    const r = parsePrometheusLine('cpu_usage 0.42 1697000000000')
    expect(r?.kind).toBe('sample')
    if (r?.kind === 'sample') {
      expect(r.sample.value).toBe(0.42)
      expect(r.sample.timestamp).toBe(1697000000000)
    }
  })

  it('支持指标名中的冒号（Prom 命名空间约定）', () => {
    const r = parsePrometheusLine(':foo:bar_total 1')
    expect(r?.kind).toBe('sample')
  })

  it('解析 HELP / TYPE 元数据行', () => {
    expect(parsePrometheusLine('# HELP gateway_queue_length Current data queue depth')).toEqual({
      kind: 'help',
      name: 'gateway_queue_length',
      text: 'Current data queue depth',
    })
    expect(parsePrometheusLine('# TYPE gateway_queue_length gauge')).toEqual({
      kind: 'type',
      name: 'gateway_queue_length',
      metricType: 'gauge',
    })
  })

  it('HELP 描述含多空格时只在第一个空格后整体保留', () => {
    const r = parsePrometheusLine('# HELP m Total  successful   uplink ')
    expect(r).toEqual({ kind: 'help', name: 'm', text: 'Total  successful   uplink' })
  })

  it('空行 / 普通注释 / 缺 token 的 HELP / 伪 HELP 均返回 null', () => {
    expect(parsePrometheusLine('')).toBeNull()
    expect(parsePrometheusLine('   ')).toBeNull()
    expect(parsePrometheusLine('# some other comment')).toBeNull()
    expect(parsePrometheusLine('# HELP only_name')).toBeNull()
    expect(parsePrometheusLine('# TYPE only_name')).toBeNull()
    expect(parsePrometheusLine('# HELPER x desc')).toBeNull()
  })

  it('数值非法（NaN/Inf/非数字）的样本行返回 null', () => {
    expect(parsePrometheusLine('foo NaN')).toBeNull()
    expect(parsePrometheusLine('foo +Inf')).toBeNull()
    expect(parsePrometheusLine('foo abc')).toBeNull()
  })

  it('指标名非法的行返回 null', () => {
    expect(parsePrometheusLine('123 456')).toBeNull()
    expect(parsePrometheusLine('a-b 1')).toBeNull()
  })

  it('非法时间戳返回不带 timestamp 的样本（与旧解析器行为一致）', () => {
    const r = parsePrometheusLine('foo 1 notanumber')
    expect(r?.kind).toBe('sample')
    if (r?.kind === 'sample') {
      expect(r.sample.value).toBe(1)
      expect(r.sample.timestamp).toBeUndefined()
    }
  })

  it('label 值中的转义引号原样保留', () => {
    const r = parsePrometheusLine('foo{a="x\\"y"} 1')
    expect(r?.kind).toBe('sample')
    if (r?.kind === 'sample') {
      expect(r.sample.labels.a).toBe('x\\"y')
    }
  })

  it('CRLF 与首尾空白不影响解析', () => {
    expect(parsePrometheusLine('\tfoo 1\r')?.kind).toBe('sample')
  })
})

describe('parsePrometheusLines', () => {
  it('完整文本按顺序产出元数据与样本，非法行被丢弃', () => {
    const text = [
      '# HELP foo help text',
      '# TYPE foo counter',
      'foo{a="1"} 2',
      'foo{a="2"} 3',
      '',
      '# unknown comment',
      'garbage-line',
    ].join('\n')
    const lines = parsePrometheusLines(text)
    expect(lines.map((l) => l.kind)).toEqual(['help', 'type', 'sample', 'sample'])
    const samples = lines.filter((l): l is Extract<typeof l, { kind: 'sample' }> => l.kind === 'sample')
    // 同名两条带标签样本不聚合（分词器层保持逐行）
    expect(samples[0]!.sample.value).toBe(2)
    expect(samples[1]!.sample.value).toBe(3)
  })

  it('null / undefined 入参安全降级为空数组', () => {
    // @ts-expect-error 运行时兜底测试
    expect(parsePrometheusLines(null)).toEqual([])
    // @ts-expect-error 运行时兜底测试
    expect(parsePrometheusLines(undefined)).toEqual([])
  })
})
