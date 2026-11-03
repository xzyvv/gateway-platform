/**
 * 抓包页「存为回放会话」组装逻辑（唯一调用方：PacketCapture）。
 *
 * 注意两点语义，缺了都会让保存出来的会话回放结果失真：
 * 1. 响应帧一并保留：后端按相邻「请求 → 其后第一个响应」配对比较，
 *    只存请求帧会让期望响应为空，健康设备回放时全部判 mismatch；
 * 2. 时间戳归零：首帧记 0ms，回放按帧间隔下发而非绝对时间。
 */
import type { ReplayPacket, ReplaySession } from '@/api/protocolTools/types'

export interface ReplayFrameInput {
  /** 帧捕获时刻（epoch ms） */
  time: number
  direction: 'request' | 'response'
  hex: string
  parsed: string
}

export function buildReplaySession(name: string, deviceName: string, frames: ReplayFrameInput[]): ReplaySession {
  const sorted = [...frames].sort((a, b) => a.time - b.time)
  const t0 = sorted[0]?.time ?? 0
  const packets: ReplayPacket[] = sorted.map((f, i) => ({
    id: 'f' + (i + 1),
    time: Math.max(0, Math.round(f.time - t0)),
    direction: f.direction,
    hex: f.hex,
    parsed: f.parsed,
  }))
  const durationMs = packets.length ? packets[packets.length - 1]!.time : 0
  return {
    id: 'u_' + Date.now().toString(36),
    name,
    deviceName,
    durationMs,
    packetCount: packets.length,
    createdAt: Date.now(),
    packets,
  }
}

/** 默认会话名：抓包会话 + 当前时刻（本地时区，冒号替换掉避免路径/显示问题） */
export function defaultSessionName(): string {
  const d = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  return `抓包会话 ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}
