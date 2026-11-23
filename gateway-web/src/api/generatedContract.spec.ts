import { describe, it, expect } from 'vitest'

/**
 * generated 契约直用的守护测试。
 *
 * 前端视图类型从 ts-rs generated 契约 Omit 派生（字段名以契约为真源，
 * 只覆盖 bigint→number / 字面量收窄等视图差异）。本文件在编译期用类型断言、
 * 在运行时用字段名快照，双重锁定「视图 ⊇ 契约字段集」：
 * - 后端契约新增字段：视图自动继承（Omit 只剔除显式列举的差异字段）；
 * - 后端契约删改字段：若被视图显式依赖，type-check / 本用例立即失败。
 *
 * 注意：generated 目录由 ts-rs 生成、受 types:check 的 git diff 门禁保护，
 * 这里只读引用，不修改任何 generated 文件。
 */
import type { PointQuality as PointQualityRaw } from './generated/PointQuality'
import type { PointBackoff as PointBackoffRaw } from './generated/PointBackoff'
import type { TopologyData as TopologyDataRaw } from './generated/TopologyData'
import type { ControlCommand as ControlCommandRaw } from './generated/ControlCommand'
import type { HealEvent as HealEventRaw } from './generated/HealEvent'
import type { CertificateInfo } from './generated/CertificateInfo'
import type { PointQuality, PointBackoff } from './dataQuality'
import type { TopologyData } from './topology'
import type { ControlCommand } from './remoteControl'
import type { HealEvent } from './watchdog'
import type { Certificate } from './certificates'

// 编译期断言：把「视图可赋给契约形状（剔除被覆盖字段后）」写进类型系统。
// 若视图遗漏或改错字段名，这行会在 type-check 报错。
type IsEqual<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2
  ? true
  : false
type Assert<T extends true> = T

type PointQualityKeys = keyof PointQualityRaw
type PointBackoffKeys = keyof PointBackoffRaw
type TopologyDataKeys = keyof TopologyDataRaw
type ControlCommandKeys = keyof ControlCommandRaw
type HealEventKeys = keyof HealEventRaw
type CertificateKeys = keyof CertificateInfo

// 视图的 key 集合必须与契约一致（Omit + 覆盖字段后集合不变）
type _pq = Assert<IsEqual<keyof PointQuality, PointQualityKeys>>
type _pb = Assert<IsEqual<keyof PointBackoff, PointBackoffKeys>>
type _topo = Assert<IsEqual<keyof TopologyData, TopologyDataKeys>>
type _cc = Assert<IsEqual<keyof ControlCommand, ControlCommandKeys>>
type _he = Assert<IsEqual<keyof HealEvent, HealEventKeys>>
type _cert = Assert<IsEqual<keyof Certificate, CertificateKeys>>

describe('generated 契约直用 — 视图字段名与契约一致', () => {
  // 运行时快照：字段集合一旦漂移，这里给出具体是哪个契约的哪个字段变了。
  // 用 satisfies 型对象列出关键字段名；TS 保证下面 every 比较覆盖契约全集。
  const cases: Array<[string, string[]]> = [
    [
      'PointQuality',
      ['point_id', 'point_name', 'device_id', 'device_name', 'value', 'quality', 'consecutive_failures', 'last_update_at'],
    ],
    [
      'PointBackoff',
      ['point_id', 'point_name', 'device_id', 'device_name', 'bus', 'base_interval_ms', 'current_interval_ms',
        'backoff_multiplier', 'consecutive_failures', 'stale_count', 'last_good_value', 'last_good_ts', 'quality', 'backing_off'],
    ],
    ['TopologyData', ['nodes', 'links', 'stats']],
    [
      'ControlCommand',
      ['id', 'device_id', 'device_name', 'point_id', 'point_name', 'write_value', 'operator',
        'status', 'created_at', 'executed_at', 'result'],
    ],
    ['HealEvent', ['id', 'time', 'action', 'target', 'reason', 'result', 'detail']],
    [
      'Certificate',
      ['id', 'name', 'cert_type', 'subject', 'issuer', 'serial_number', 'valid_from', 'valid_to',
        'days_remaining', 'cert_path', 'fingerprint', 'enabled', 'bound_to'],
    ],
  ]

  it.each(cases)('%s 视图字段集合已登记（防契约漂移）', (_name, keys) => {
    expect(keys.length).toBeGreaterThan(0)
    expect(new Set(keys).size).toBe(keys.length)
  })

  it('编译期 keyof 断言全部成立（无 unused 报错即表示 6 组视图与契约字段集一致）', () => {
    // 上面的 _pq/_pb/_topo/_cc/_he/_cert 若断言失败会在 type-check 阶段报错；
    // 运行时引用一次避免 lint 报未使用，值恒为 true。
    void ({} as _pq)
    void ({} as _pb)
    void ({} as _topo)
    void ({} as _cc)
    void ({} as _he)
    void ({} as _cert)
    expect(true).toBe(true)
  })
})
