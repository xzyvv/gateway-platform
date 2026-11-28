/**
 * 重连退避预设：MQTT 客户端与总线（TCP）共用同一套档位。
 *
 * DD-10：`standard` 不再手写，直接取自后端 `MqttConfig` 的 serde 默认值
 * （由 `npm run types:export` 生成的同源常量），保证「不动这个下拉」不会改变
 * 升级前的重连节奏。总线自己的默认退避与 MQTT 不同（1/2/5/10/30 + 20%），
 * 由调用方从 `bus.reconnect_backoff` / `defaultBusReconnectBackoff()` 读取，
 * 不再拿 MQTT 的标准档给总线兜底。
 */
import type { MqttConfig } from '@/api/types'
import { DEFAULT_MQTT } from '@/api/generated/configDefaults.generated'

export type Backoff = MqttConfig['reconnect_backoff']
export type BackoffPreset = 'standard' | 'fast' | 'conservative' | 'custom'

export const BACKOFF_PRESETS: Record<Exclude<BackoffPreset, 'custom'>, Backoff> = {
  standard: {
    seconds: [...DEFAULT_MQTT.reconnect_backoff.seconds],
    jitter_ratio: DEFAULT_MQTT.reconnect_backoff.jitter_ratio,
  },
  fast: { seconds: [1, 1, 2, 3, 5, 10], jitter_ratio: 0.2 },
  conservative: { seconds: [5, 15, 30, 60, 120, 300], jitter_ratio: 0.5 },
}

export const BACKOFF_LABELS: Record<BackoffPreset, string> = {
  standard: '标准',
  fast: '快速',
  conservative: '保守',
  custom: '自定义',
}

/** 下拉选项文案；带上秒数与抖动，现场不必先选再猜 */
const standardLabel = `标准（${BACKOFF_PRESETS.standard.seconds.join('/')} 秒，抖动 ${Math.round(
  BACKOFF_PRESETS.standard.jitter_ratio * 100,
)}%）`

export const BACKOFF_OPTIONS: ReadonlyArray<{ value: BackoffPreset; label: string }> = [
  { value: 'standard', label: standardLabel },
  { value: 'fast', label: '快速（1/1/2/3/5/10 秒，抖动 20%）' },
  { value: 'conservative', label: '保守（5/15/30/60/120/300 秒，抖动 50%）' },
  { value: 'custom', label: '自定义（配置文件档位，选此项不改写）' },
]

/** 两档是否等价（秒数逐项相等 + 抖动浮点容差） */
export function sameBackoff(a: Backoff, b: Backoff): boolean {
  return (
    a.seconds.length === b.seconds.length &&
    a.seconds.every((s, i) => s === b.seconds[i]) &&
    Math.abs(a.jitter_ratio - b.jitter_ratio) < 1e-9
  )
}

/** 反查当前配置命中的预设；不在任何预设里则为 custom */
export function presetOf(cur: Backoff): BackoffPreset {
  const hit = (Object.keys(BACKOFF_PRESETS) as Array<Exclude<BackoffPreset, 'custom'>>).find((k) =>
    sameBackoff(cur, BACKOFF_PRESETS[k]),
  )
  return hit ?? 'custom'
}

/** 生成预设对应的新配置；custom 返回 null 表示「不改写现场手调的档位」 */
export function applyPreset(preset: BackoffPreset): Backoff | null {
  if (preset === 'custom') return null
  const p = BACKOFF_PRESETS[preset]
  return { seconds: [...p.seconds], jitter_ratio: p.jitter_ratio }
}
