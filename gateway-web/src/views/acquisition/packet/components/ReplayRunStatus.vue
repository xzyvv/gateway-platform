<template>
  <!-- P0-1b 流式进度：逐帧落地，进度条显示「已完成 N / total」 -->
  <div v-if="playing && total > 0" class="progress-line" aria-live="polite">
    <el-progress :percentage="percentage" :stroke-width="8" :show-text="false" style="flex:1" />
    <span class="progress-text">已完成 {{ doneCount }} / {{ total }}</span>
  </div>

  <!-- DD-03：终态提示。aborted / failed / stopped 不能只显示「一致 N」让人误以为已跑完 -->
  <el-alert
    v-if="terminal && terminal !== 'completed' && !playing"
    :type="terminal === 'aborted' || terminal === 'stopped' ? 'warning' : 'error'"
    :title="terminalTitle"
    :description="terminalDescription"
    show-icon
    :closable="false"
    class="terminal-alert"
  />

  <!-- 回放结果汇总 -->
  <div v-if="result" class="summary" aria-live="polite">
    <el-tag type="info" size="small">共 {{ result.total }} 帧</el-tag>
    <el-tag type="success" size="small">一致 {{ result.match }}</el-tag>
    <el-tag type="warning" size="small">不一致 {{ result.mismatch }}</el-tag>
    <el-tag type="danger" size="small">失败 {{ result.failed }}</el-tag>
    <span class="duration">耗时 {{ formatElapsed(result.finishedAtMs - result.startedAtMs) }} · 总线 {{ result.bus }}</span>
  </div>
</template>

<script setup lang="ts">
/**
 * 报文回放「运行状态条」（从 PacketReplay.vue 拆出）
 *
 * 职责：流式回放中的逐帧进度条（已完成 N / total），以及回放结束后的
 * 一致/不一致/失败汇总与耗时。纯展示区块，数据全部来自 usePacketReplay。
 *
 * 为什么进度与汇总放在同一块：两者互斥出现（执行中看进度、结束后看汇总），
 * 同处一屏同一位置，切换时不会造成卡片跳动。
 *
 * Props:
 * - playing：回放进行中
 * - doneCount：已收到逐帧结果数（进度条分子）
 * - total：流式 started 给出的计划总帧数（0 表示未开始/阻塞模式）
 * - percentage：进度百分比
 * - result：回放汇总结果（null 表示尚未执行）
 */
import { computed } from 'vue'
import type { ReplayExecuteResult } from '@/api/protocolTools'
import { formatElapsed } from '@/utils/format'

const props = defineProps<{
  /** 回放进行中 */
  playing: boolean
  /** 已收到逐帧结果数 */
  doneCount: number
  /** 计划总帧数 */
  total: number
  /** 进度百分比 */
  percentage: number
  /** 回放汇总结果 */
  result: ReplayExecuteResult | null
  /** DD-03：服务端/用户给出的终态，空串表示尚未执行 */
  terminal?: '' | 'completed' | 'aborted' | 'failed' | 'stopped'
  /** 终态附加说明（失败原因等） */
  terminalError?: string
}>()

const terminalTitle = computed(() => {
  switch (props.terminal) {
    case 'aborted': return '回放未跑完（服务端已中止）'
    case 'failed': return '回放失败'
    case 'stopped': return '回放已停止'
    default: return ''
  }
})

const terminalDescription = computed(() => {
  const done = props.result?.total ?? props.doneCount
  const planned = props.result?.planned || props.total
  const base = planned > 0 ? `已执行 ${done} / ${planned} 帧` : `已执行 ${done} 帧`
  if (props.terminal === 'aborted') return `${base}（达到单次回放时长上限）。下面只统计已执行部分，未跑完的帧未验证。`
  if (props.terminal === 'stopped') return `${base}（由用户停止）。未跑完的帧未验证。`
  return props.terminalError ? `${base}：${props.terminalError}` : base
})
</script>

<style scoped>
.summary { display: flex; gap: 8px; align-items: center; margin-bottom: 12px; flex-wrap: wrap; }
.terminal-alert { margin-bottom: 12px; }
.progress-line { display: flex; gap: 12px; align-items: center; margin-bottom: 12px; }
.progress-text { font-size: 0.75rem; color: var(--el-text-color-secondary); white-space: nowrap; }
.duration { font-size: 0.8125rem; color: var(--el-text-color-secondary); }
</style>
