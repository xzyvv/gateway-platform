/**
 * 组件健康状态词表的判定契约。
 *
 * 这里锁死的是「后端真实状态词 → 中文名 / 严重度 / 徽章色」的映射：
 * 词表一旦漂移，仪表盘就会出现「表格红徽章、汇总黄底色」这种同页矛盾，
 * 或者把正常网关判成「N 个组件未在运行」。
 *
 * 变异验证：
 *   - 把 ok 的 severity 从 'ok' 改成 'warning' → 「ok 不进未就绪」失败
 *   - 把 down 的 severity 从 'error' 改成 'warning' → 「down 计入异常」失败
 *   - 删掉 TAG_OVERRIDE.backpressure → 「backpressure 为 warning 色」失败
 *   - 把未登记状态的 severity 改成 'ok' → 「未知状态不被静默吞掉」失败
 */
import { describe, it, expect } from 'vitest'
import {
  healthComponentLabel,
  healthRowsFrom,
  healthSeverity,
  healthSnapshotDegraded,
  healthStatusInfo,
  healthStatusLabel,
  healthStatusTag,
  healthWorstSeverity,
} from './healthStatus'

describe('healthStatusInfo', () => {
  it.each([
    // 后端真实取值
    ['ok', '正常', 'ok', 'success'],
    ['down', '不可用', 'error', 'danger'],
    ['backpressure', '队列积压', 'warning', 'warning'],
    ['degraded', '降级运行', 'warning', 'warning'],
    // 历史取值（兼容）
    ['running', '运行中', 'ok', 'success'],
    ['stopped', '已停止', 'warning', 'warning'],
    ['starting', '启动中', 'warning', 'warning'],
    ['error', '异常', 'error', 'danger'],
  ])('%s → %s / %s / %s', (status, label, severity, tag) => {
    expect(healthStatusInfo(status)).toEqual({ label, severity, tag })
  })

  it('大小写与空白不敏感', () => {
    expect(healthStatusLabel(' OK ')).toBe('正常')
    expect(healthSeverity('Down')).toBe('error')
  })

  it('严重度与徽章色永远同档（不会出现红徽章配黄汇总）', () => {
    const expected: Record<string, string> = { ok: 'success', warning: 'warning', error: 'danger' }
    for (const status of ['ok', 'down', 'backpressure', 'degraded', 'running', 'stopped', 'starting', 'error', 'draining']) {
      const info = healthStatusInfo(status)
      expect(info.tag).toBe(expected[info.severity])
    }
  })

  it('未登记状态原样透出名字，但不静默吞掉（按需要关注处理）', () => {
    const info = healthStatusInfo('draining')
    expect(info.label).toBe('draining')
    expect(info.severity).toBe('warning')
    expect(info.tag).toBe('warning')
  })

  it('空状态按未就绪处理，不报正常', () => {
    expect(healthSeverity('')).toBe('warning')
    expect(healthSeverity('   ')).toBe('warning')
  })
})

describe('healthComponentLabel', () => {
  it('后端真实组件名映射成中文', () => {
    expect(healthComponentLabel('process')).toBe('网关主进程')
    expect(healthComponentLabel('mqtt')).toBe('MQTT 客户端')
    expect(healthComponentLabel('queue')).toBe('采集队列')
    expect(healthComponentLabel('ota')).toBe('OTA 升级')
  })

  it('历史组件名仍可显示', () => {
    expect(healthComponentLabel('web_server')).toBe('Web 服务')
    expect(healthComponentLabel('data_collector')).toBe('数据采集器')
  })

  it('未登记组件名原样透出', () => {
    expect(healthComponentLabel('brand_new')).toBe('brand_new')
  })
})

describe('healthStatusTag', () => {
  it('与 healthStatusInfo 同源', () => {
    expect(healthStatusTag('down')).toBe('danger')
    expect(healthStatusTag('backpressure')).toBe('warning')
  })
})

describe('healthSnapshotDegraded', () => {
  it('全 ok 时不算降级（不按后端聚合 status）', () => {
    expect(
      healthSnapshotDegraded(
        { process: 'ok', mqtt: 'ok', queue: 'ok', ota: 'ok' },
        'degraded',
      ),
    ).toBe(false)
  })

  it('存在 down / backpressure / degraded 时算降级', () => {
    expect(healthSnapshotDegraded({ mqtt: 'down' }, 'degraded')).toBe(true)
    expect(healthSnapshotDegraded({ queue: 'backpressure' }, 'degraded')).toBe(true)
    expect(healthSnapshotDegraded({ ota: 'degraded' }, 'degraded')).toBe(true)
  })

  it('没有组件明细时回退到后端聚合 status', () => {
    expect(healthSnapshotDegraded({}, 'ok')).toBe(false)
    expect(healthSnapshotDegraded({}, 'degraded')).toBe(true)
    expect(healthSnapshotDegraded(null, undefined)).toBe(false)
  })
})

describe('healthRowsFrom', () => {
  it('把 components 展开成表格行', () => {
    expect(healthRowsFrom({ status: 'ok', components: { mqtt: 'down', process: 'ok' } })).toEqual([
      { name: 'mqtt', status: 'down' },
      { name: 'process', status: 'ok' },
    ])
  })

  it('没有快照或没有组件明细时返回空数组（表格显示空态，不报错）', () => {
    expect(healthRowsFrom(null)).toEqual([])
    expect(healthRowsFrom(undefined)).toEqual([])
    expect(healthRowsFrom({ status: 'ok', components: {} })).toEqual([])
  })
})

describe('healthWorstSeverity', () => {
  it('取组件里最严重的一档', () => {
    expect(healthWorstSeverity({ mqtt: 'ok', queue: 'ok' })).toBe('ok')
    expect(healthWorstSeverity({ mqtt: 'ok', queue: 'backpressure' })).toBe('warning')
    expect(healthWorstSeverity({ queue: 'backpressure', mqtt: 'down' })).toBe('error')
  })

  it('没有组件明细时按后端聚合 status 兜底', () => {
    expect(healthWorstSeverity({}, 'ok')).toBe('ok')
    expect(healthWorstSeverity({}, 'degraded')).toBe('warning')
    expect(healthWorstSeverity(null, undefined)).toBe('ok')
  })
})
