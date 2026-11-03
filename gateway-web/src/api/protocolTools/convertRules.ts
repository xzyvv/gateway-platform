/**
 * 协议转换规则 API
 * GET    /api/protocol-tools/convert/rules
 * POST   /api/protocol-tools/convert/rules
 * PUT    /api/protocol-tools/convert/rules/:id
 * DELETE /api/protocol-tools/convert/rules/:id
 * POST   /api/protocol-tools/convert/rules/:id/toggle
 */
import { request } from '../index'
import type { ConvertRule } from './types'
import type { ConvertRule as ConvertRuleRaw } from '../generated'
import { toConvertRule } from './types'

/**
 * POST/PUT 请求体：与后端 ConvertRule 同构（snake_case）。
 * 唯一差异：last_converted_at_ms 在 Rust 侧是 Option<i64>（线网接受 number|null），
 * ts-rs 的 #[ts(type = "number")] 把它注记成了 number，这里按线网真实形态放宽。
 */
type ConvertRuleWrite = Omit<ConvertRuleRaw, 'last_converted_at_ms'> & {
  last_converted_at_ms: number | null
}

/**
 * View(camelCase) → 请求体(snake_case)。
 * 后端 POST/PUT 均反序列化为完整 ConvertRule（V1 内存态）：
 * id 传 ''、created_at_ms 传 0 作为占位，由后端补全（PUT 时后端保留原 created_at_ms）。
 */
function toRawBody(rule: Partial<ConvertRule>, id: string): ConvertRuleWrite {
  return {
    id,
    name: rule.name ?? '',
    source_proto: rule.sourceProto ?? '',
    source_point_id: rule.sourcePointId ?? '',
    source_point_name: rule.sourcePointName ?? '',
    target_proto: rule.targetProto ?? '',
    target_tag: rule.targetTag ?? '',
    transform: rule.transform ?? '',
    scale: rule.scale ?? null,
    offset: rule.offset ?? null,
    enum_map: rule.enumMap ?? null,
    enabled: rule.enabled ?? false,
    last_converted_at_ms: rule.lastConvertedAt ?? null,
    last_value: rule.lastValue ?? null,
    created_at_ms: 0,
  }
}

export function listConvertRules(): Promise<ConvertRule[]> {
  return request<ConvertRuleRaw[]>({ url: '/api/protocol-tools/convert/rules', method: 'GET' }).then((arr) => arr.map(toConvertRule))
}
export function createConvertRule(rule: Omit<ConvertRule, 'id' | 'createdAt'>): Promise<ConvertRule> {
  const body = toRawBody(rule, '')
  return request<ConvertRuleRaw>({ url: '/api/protocol-tools/convert/rules', method: 'POST', data: body }).then(toConvertRule)
}
export function updateConvertRule(id: string, patch: Partial<ConvertRule>): Promise<void> {
  const body = toRawBody(patch, id)
  return request<void>({ url: '/api/protocol-tools/convert/rules/' + encodeURIComponent(id), method: 'PUT', data: body })
}
export function deleteConvertRule(id: string): Promise<void> {
  return request<void>({ url: '/api/protocol-tools/convert/rules/' + encodeURIComponent(id), method: 'DELETE' })
}
export function toggleConvertRule(id: string): Promise<boolean> {
  return request<boolean>({ url: '/api/protocol-tools/convert/rules/' + encodeURIComponent(id) + '/toggle', method: 'POST' })
}
