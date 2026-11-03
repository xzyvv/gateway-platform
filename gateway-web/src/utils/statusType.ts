// 全站状态 → 徽章色（Element Plus <el-tag> 的 type）的唯一映射。
//
// 背景：此前各页面各写各的 xxxType()（healthTagType / ntpTagType /
// auditStatusType / commsQualityType ...），同一状态在不同页取色不一致——
// 「离线」在一处是灰的（info）、另一处是红的（danger），用户对不上号。
//
// 这里收敛为一份语义映射：调用方只给状态原文（中文或英文），拿到的永远是同一套颜色。

/** Element Plus <el-tag> type 接受的字面量联合类型 */
export type TagType = 'success' | 'warning' | 'info' | 'danger' | 'primary'

/**
 * 精确匹配表（状态原文 → 颜色）。
 *
 * 放在子串匹配之前：像「非正常」「部分成功」这类包含关系，
 * 直接做子串判断会命中错误的档位。
 */
const EXACT: Record<string, TagType> = {
  // 成功 / 在线
  正常: 'success', 在线: 'success', 成功: 'success', 已连接: 'success',
  运行中: 'success', 启用: 'success', 已启用: 'success', 通过: 'success',
  良好: 'success', 同步: 'success', 已同步: 'success', 就绪: 'success',
  可用: 'success', 有效: 'success', 健康: 'success',
  online: 'success', connected: 'success', active: 'success', running: 'success',
  enabled: 'success', success: 'success', ok: 'success', pass: 'success',
  passed: 'success', normal: 'success', good: 'success', ready: 'success',
  valid: 'success', synced: 'success', up: 'success', healthy: 'success',

  // 告警 / 降级
  告警: 'warning', 警告: 'warning', 预警: 'warning', 降级: 'warning',
  待处理: 'warning', 处理中: 'warning', 部分: 'warning', 过期: 'warning',
  即将过期: 'warning', 不确定: 'warning', 波动: 'warning', 忙: 'warning',
  降级运行: 'warning',
  warning: 'warning', warn: 'warning', degraded: 'warning', pending: 'warning',
  partial: 'warning', stale: 'warning', expired: 'warning', expiring: 'warning',
  uncertain: 'warning', unstable: 'warning', busy: 'warning', old: 'warning',

  // 失败 / 离线
  失败: 'danger', 离线: 'danger', 错误: 'danger', 断开: 'danger',
  故障: 'danger', 异常: 'danger', 失效: 'danger', 无效: 'danger',
  超时: 'danger', 中止: 'danger', 拒绝: 'danger', 崩溃: 'danger',
  error: 'danger', failed: 'danger', failure: 'danger', fail: 'danger',
  offline: 'danger', disconnected: 'danger', down: 'danger', fault: 'danger',
  invalid: 'danger', critical: 'danger', fatal: 'danger', timeout: 'danger',
  reject: 'danger', aborted: 'danger', bad: 'danger',

  // 未知 / 未启用
  未知: 'info', 未配置: 'info', 未启用: 'info', 停用: 'info', 禁用: 'info',
  其他: 'info', 空闲: 'info', 无: 'info',
  unknown: 'info', none: 'info', disabled: 'info', inactive: 'info',
  idle: 'info', not_configured: 'info', stopped: 'info',
}

/**
 * 子串匹配兜底：后端新增的状态词没进精确表时，至少能给出一个合理档位。
 *
 * 顺序 = 优先级：失败类先判（避免「部分失败」被当成成功），
 * 再告警类，最后才是成功类。
 */
const SUBSTRING_ORDER: { keys: string[]; type: TagType }[] = [
  {
    keys: ['失败', '离线', '错误', '断开', '故障', '异常', '失效', '无效', '超时', '崩溃', '中止'],
    type: 'danger',
  },
  {
    keys: ['告警', '警告', '预警', '降级', '待处理', '部分', '过期', '不确定', '波动', '忙'],
    type: 'warning',
  },
  {
    keys: ['正常', '在线', '成功', '已连接', '运行', '启用', '通过', '良好', '同步', '就绪', '可用', '有效'],
    type: 'success',
  },
]

/**
 * 取状态对应的徽章颜色。
 *
 * @param status 状态原文（中文或英文，大小写不敏感）；null/undefined/空串按「未知」处理
 * @returns Element Plus tag 的 type
 */
export function statusTagType(status: string | number | null | undefined): TagType {
  if (status === null || status === undefined) return 'info'
  const raw = String(status).trim()
  if (!raw) return 'info'

  const exact = EXACT[raw] ?? EXACT[raw.toLowerCase()]
  if (exact) return exact

  const lower = raw.toLowerCase()
  for (const group of SUBSTRING_ORDER) {
    for (const key of group.keys) {
      if (lower.includes(key.toLowerCase())) return group.type
    }
  }
  return 'info'
}
