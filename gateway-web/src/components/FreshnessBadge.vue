<template>
  <span class="freshness" :class="freshnessClass">
    {{ freshnessText }}
  </span>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useSharedTick } from '@/composables/core/useSharedTick'

const props = defineProps<{ timestamp: number }>()

// 内部 tick：仅本组件重渲染，不触发父表格整体刷新
const tick = ref(0)

function elapsed(ts: unknown): number {
  if (ts == null) return Infinity
  const n = typeof ts === 'number' ? ts : Number(ts)
  if (!Number.isFinite(n) || n <= 0) return Infinity
  // 后端 DataPoint.timestamp 为 UTC 毫秒（gateway-core model.rs:120），
  // 直接按毫秒口径计算距今天数，避免误把毫秒当秒导致恒为负数。
  return Math.floor((Date.now() - n) / 1000)
}

/**
 * 刷新周期随数据年龄降频（P0）：
 *   - 60 秒内文案是「N 秒前」，需要秒级精度 → 1s；
 *   - 超过 60 秒文案只到分钟 → 30s 一次足够。
 * 网关控制台里绝大多数行是「几分钟前」的陈旧数据，降频后单个共享定时器
 * 就能覆盖整屏，不再每行各起一个 setInterval。
 */
function periodFor(ts: number): number {
  return elapsed(ts) <= 60 ? 1000 : 30_000
}

let currentPeriod = periodFor(props.timestamp)

const { setInterval: setTickPeriod } = useSharedTick(() => {
  tick.value++
  syncPeriod()
}, currentPeriod)

/** 年龄跨过分档线（如 61 秒、1 小时）时切档，避免一直占着 1s 的高频槽位 */
function syncPeriod(): void {
  const next = periodFor(props.timestamp)
  if (next === currentPeriod) return
  currentPeriod = next
  setTickPeriod(next)
}

// 新数据到达：立刻刷新一次文案，并按新年龄重新定档（新数据回到 1s 档）
watch(() => props.timestamp, () => {
  tick.value++
  syncPeriod()
})

const freshnessClass = computed(() => {
  // 引用 tick 以触发定时刷新
  void tick.value
  const s = elapsed(props.timestamp)
  if (s <= 5) return 'freshness-fresh'
  if (s <= 30) return 'freshness-warn'
  return 'freshness-stale'
})

const freshnessText = computed(() => {
  void tick.value
  const s = elapsed(props.timestamp)
  if (!isFinite(s)) return '—'
  if (s < 60) return `${s}秒前`
  if (s < 3600) return `${Math.floor(s / 60)}分钟前`
  return `${Math.floor(s / 3600)}小时前`
})
</script>

<style scoped>
.freshness {
  font-size: 0.6875rem;
  padding: 1px 6px;
  border-radius: var(--radius-small);
  align-self: flex-start;
}
.freshness-fresh {
  color: var(--text-success);
  background: var(--color-success-bg);
  animation: freshPulse 2s ease-in-out 3;
}
@keyframes freshPulse {
  0%, 100% { box-shadow: 0 0 0 0 rgba(var(--color-success-rgb), 0); }
  50% { box-shadow: 0 0 0 3px rgba(var(--color-success-rgb), 0.15); }
}
.freshness-warn {
  color: var(--text-warning);
  background: var(--color-warning-bg);
}
.freshness-stale {
  color: var(--text-danger);
  background: var(--color-danger-bg);
}
</style>
