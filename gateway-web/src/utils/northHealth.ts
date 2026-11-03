/**
 * 北向健康统一判定（C2）
 *
 * 现场排障第一问题永远是「上行到底通没通」。MQTT / Webhook / 对外服务 Server /
 * 透传会话的原始状态分散在三个接口，字段口径各不相同（`connected` 可能为 null、
 * Server 只有 running、透传只有会话列表），直接把原始值堆到页面上仍需人工判断。
 *
 * 这里把所有北向组件折算成**一个结论**（ok / degraded / down / idle）+ 一条问题清单，
 * 组件只负责取数与渲染，判定逻辑可脱离 Vue 单测。
 */

import type { NorthChannel, NorthServer, PassthroughSession } from '@/api/uplink'

export type NorthVerdict = 'ok' | 'degraded' | 'down' | 'idle'

export interface NorthHealthIssue {
  level: 'error' | 'warning'
  text: string
}

export interface NorthHealthStats {
  /** 上行通道总数（含已停用） */
  channelTotal: number
  /** 已启用通道数 */
  channelEnabled: number
  /** 已启用且确认已连接的通道数 */
  channelConnected: number
  /** 对外服务总数 / 运行中数量 */
  serverTotal: number
  serverRunning: number
  /** 透传会话：是否启用、当前活动会话数 */
  passthroughEnabled: boolean
  passthroughSessions: number
}

export interface NorthHealthInput {
  channels: NorthChannel[]
  servers: NorthServer[]
  passthrough: { enabled: boolean; sessions: PassthroughSession[] }
  /** 扇出订阅累计丢帧批次（aggregated.lagged_dropped_batches） */
  laggedDropped?: number
  /** 消息队列积压（aggregated.pending_messages） */
  pendingMessages?: number
}

export interface NorthHealth {
  verdict: NorthVerdict
  /** 一句话标题（页面顶部统一结论） */
  title: string
  /** 结论补充说明，解释「为什么」与「下一步看哪」 */
  summary: string
  issues: NorthHealthIssue[]
  stats: NorthHealthStats
}

/** 队列积压告警阈值：与通道卡片上 `queue-warn` 的阈值一致，避免两处口径不一致 */
export const QUEUE_BACKLOG_THRESHOLD = 100

/**
 * 把北向各组件的原始状态折算成统一健康结论。
 *
 * 判定顺序：
 * 1. 没有任何北向配置 → idle；
 * 2. 有通道但全部停用 → idle（提示用户不是故障而是未启用）；
 * 3. 已启用通道全部未连接 → down；
 * 4. 任一通道未连接 / 连接态未知 / Server 停止 / 断路器熔断 / 丢帧 / 队列积压 → degraded；
 * 5. 其余 → ok。
 */
export function computeNorthHealth(input: NorthHealthInput): NorthHealth {
  const channels = input.channels ?? []
  const servers = input.servers ?? []
  const passthrough = input.passthrough ?? { enabled: false, sessions: [] }
  const sessions = passthrough.sessions ?? []
  const laggedDropped = input.laggedDropped ?? 0
  const pendingMessages = input.pendingMessages ?? 0

  const enabledChannels = channels.filter(c => c.enabled)
  const connectedChannels = enabledChannels.filter(c => c.connected === true)
  const failedChannels = enabledChannels.filter(c => c.connected === false)
  const unknownChannels = enabledChannels.filter(c => c.connected == null)
  const runningServers = servers.filter(s => s.running)

  const stats: NorthHealthStats = {
    channelTotal: channels.length,
    channelEnabled: enabledChannels.length,
    channelConnected: connectedChannels.length,
    serverTotal: servers.length,
    serverRunning: runningServers.length,
    passthroughEnabled: !!passthrough.enabled,
    passthroughSessions: sessions.length,
  }

  const issues: NorthHealthIssue[] = []

  for (const ch of failedChannels) {
    issues.push({
      level: 'error',
      text: ch.extra?.last_error
        ? `${ch.name} 未连接：${ch.extra.last_error}`
        : `${ch.name} 未连接`,
    })
  }
  for (const ch of unknownChannels) {
    issues.push({ level: 'warning', text: `${ch.name} 连接状态未知（该通道类型不提供连接态）` })
  }
  for (const ch of enabledChannels) {
    if (ch.extra?.circuit_open) {
      issues.push({ level: 'error', text: `${ch.name} 断路器已熔断（连续失败后暂停发送）` })
    }
  }
  for (const srv of servers) {
    if (!srv.running) {
      issues.push({ level: 'error', text: `对外服务 ${srv.name} 已停止` })
    }
  }
  if (channels.length > 0 && enabledChannels.length === 0) {
    issues.push({ level: 'warning', text: '所有上行通道均已停用' })
  }
  if (laggedDropped > 0) {
    issues.push({ level: 'warning', text: `上行扇出累计丢帧 ${laggedDropped} 批次（通道处理不及）` })
  }
  if (pendingMessages > QUEUE_BACKLOG_THRESHOLD) {
    issues.push({ level: 'warning', text: `消息队列积压 ${pendingMessages} 条` })
  }

  const hasAnyConfig = channels.length > 0 || servers.length > 0
  if (!hasAnyConfig) {
    return {
      verdict: 'idle',
      title: '尚未配置上行',
      summary: '未检测到上行通道或对外服务，请先在「通道转发 / 上送模板」中完成配置。',
      issues,
      stats,
    }
  }

  if (enabledChannels.length === 0) {
    return {
      verdict: 'idle',
      title: '上行未启用',
      summary:
        servers.length > 0 && runningServers.length === servers.length
          ? '上行通道均已停用，但对外服务正常运行；如需上行数据请启用通道。'
          : '上行通道均已停用，当前不会向上位系统发送任何数据。',
      issues,
      stats,
    }
  }

  if (connectedChannels.length === 0 && failedChannels.length > 0) {
    return {
      verdict: 'down',
      title: '上行中断',
      summary: '所有已启用的上行通道均未连接，数据无法送达上位系统。',
      issues,
      stats,
    }
  }

  if (issues.length > 0) {
    return {
      verdict: 'degraded',
      title: '上行部分异常',
      summary: `${connectedChannels.length}/${enabledChannels.length} 个上行通道已连接，存在待处理问题。`,
      issues,
      stats,
    }
  }

  return {
    verdict: 'ok',
    title: '上行正常',
    summary: `${connectedChannels.length}/${enabledChannels.length} 个上行通道已连接${servers.length > 0 ? `，${runningServers.length}/${servers.length} 个对外服务运行中` : ''}${sessions.length > 0 ? `，透传会话 ${sessions.length} 个` : ''}。`,
    issues,
    stats,
  }
}

/** 结论对应的 `el-alert` 类型（注意 el-alert 用 error，不是 el-tag 的 danger） */
export function verdictAlertType(verdict: NorthVerdict): 'success' | 'warning' | 'error' | 'info' {
  switch (verdict) {
    case 'ok':
      return 'success'
    case 'degraded':
      return 'warning'
    case 'down':
      return 'error'
    default:
      return 'info'
  }
}
