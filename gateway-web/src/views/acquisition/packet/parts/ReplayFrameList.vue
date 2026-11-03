<template>
  <div class="replay-list" aria-label="回放报文列表">
    <template v-for="p in packets" :key="p.id">
      <div class="packet" :class="[p.direction, statusClass(p.id)]">
        <div class="p-time">{{ (p.time/1000).toFixed(2) }}s</div>
        <el-tag size="small" :type="p.direction === 'request' ? 'primary' : 'success'">{{ p.direction === 'request' ? '请求' : '响应' }}</el-tag>
        <code class="hex">{{ p.hex || '(空)' }}</code>
        <span class="parsed">{{ p.parsed }}</span>
        <el-tag v-if="frameOf(p.id)" size="small" :type="statusTagType(frameOf(p.id)!.status)">
          {{ statusLabel(frameOf(p.id)!.status) }}
        </el-tag>
      </div>
      <!-- 帧级判定详情：只在有不一致/失败时展开实际响应，避免刷屏 -->
      <div v-if="frameOf(p.id) && frameOf(p.id)!.status !== 'match'" class="frame-detail">
        <span>实际响应: <code>{{ frameOf(p.id)!.actualResponseHex || '(无响应)' }}</code></span>
        <span>耗时 {{ frameOf(p.id)!.latencyMs }}ms</span>
        <span v-if="frameOf(p.id)!.error" class="err-text">{{ withDiagHint(frameOf(p.id)!.error!) }}</span>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
/**
 * 报文回放「帧列表」（从 PacketReplay.vue 拆出）
 *
 * 职责：按录制顺序渲染每一帧（时间、方向、HEX、解析值、判定徽章），
 * 并在不一致/失败时展开实际响应、耗时与诊断建议（一致帧不展开，避免刷屏）。
 *
 * 与页面主组件的分工：逐帧结果由 usePacketReplay 合并成 framesById 传入，
 * 本组件只负责查表与展示，不再自行推导执行状态。
 *
 * Props:
 * - packets：录制会话里的报文序列
 * - frames：packetId → 逐帧结果（执行中为流式中间态，结束后为全量汇总）
 */
import { withDiagHint } from '@/utils/diagHints'
import type { ReplayPacket, ReplayFrameResult } from '@/api/protocolTools'
import { statusLabel, statusTagType } from '@/composables/acquisition/usePacketReplay'

const props = defineProps<{
  /** 录制会话里的报文序列 */
  packets: ReplayPacket[]
  /** packetId → 逐帧结果 */
  frames: Map<string, ReplayFrameResult>
}>()

function frameOf(packetId: string): ReplayFrameResult | undefined {
  return props.frames.get(packetId)
}

function statusClass(packetId: string): string {
  const r = frameOf(packetId)
  return r ? `st-${r.status}` : ''
}
</script>

<style scoped>
.replay-list { max-height: 420px; overflow-y: auto; }
.packet { display: flex; gap: 10px; align-items: center; padding: 6px 10px; border-bottom: 1px solid var(--el-border-color-lighter); font-family: monospace; font-size: 0.8125rem; }
.packet.st-match { background: var(--el-color-success-light-9); }
.packet.st-mismatch { background: var(--el-color-warning-light-9); border-left: 3px solid var(--el-color-warning); }
.packet.st-timeout { background: var(--el-color-info-light-9); }
.packet.st-error { background: var(--el-color-danger-light-9); border-left: 3px solid var(--el-color-danger); }
.frame-detail { display: flex; gap: 16px; padding: 4px 10px 8px 72px; font-size: 0.75rem; color: var(--el-text-color-secondary); border-bottom: 1px solid var(--el-border-color-lighter); flex-wrap: wrap; }
.err-text { color: var(--text-danger); }
.p-time { color: var(--el-text-color-secondary); min-width: 50px; }
.hex { color: var(--el-color-primary); }
.parsed { color: var(--el-text-color-regular); font-family: sans-serif; }
</style>
