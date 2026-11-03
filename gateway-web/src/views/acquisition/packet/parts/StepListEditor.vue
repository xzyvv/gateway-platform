<template>
  <empty-state
    v-if="!activeSequence || activeSequence.steps.length === 0"
    description="添加步骤，或从预置添加一条开始"
    :image-size="80"
  />
  <div v-else class="step-list">
    <div
      v-for="(s, i) in activeSequence?.steps ?? []"
      :key="s.id"
      class="step-item"
      :class="stepClass(s.id, i)"
    >
      <div class="step-num">{{ i + 1 }}</div>
      <div class="step-body">
        <div class="step-row">
          <el-input
            :model-value="s.name"
            size="small"
            class="step-name"
            placeholder="步骤名（可选）"
            :aria-label="`步骤 ${i + 1} 名称`"
            :disabled="running"
            @update:model-value="onStepName(s.id, $event)"
          />
          <el-input-number
            :model-value="s.timeoutMs"
            size="small"
            class="step-timeout"
            :min="MIN_TIMEOUT_MS"
            :max="MAX_TIMEOUT_MS"
            :step="100"
            controls-position="right"
            :aria-label="`步骤 ${i + 1} 超时时间`"
            :disabled="running"
            @update:model-value="onStepTimeout(s.id, $event)"
          />
          <el-switch
            :model-value="s.enabled"
            size="small"
            :disabled="running"
            @update:model-value="onStepEnabled(s.id, $event)"
          />
        </div>
        <el-input
          :model-value="s.hex"
          size="small"
          class="step-hex"
          placeholder="HEX，如 01 03 00 00 00 02"
          :aria-label="`步骤 ${i + 1} 报文`"
          :disabled="running"
          @update:model-value="onStepHex(s.id, $event)"
        />
        <div class="step-tools">
          <el-button size="small" link :disabled="running || i === 0" @click="moveStep(s.id, -1)">上移</el-button>
          <el-button
            size="small"
            link
            :disabled="running || i === (activeSequence?.steps.length ?? 0) - 1"
            @click="moveStep(s.id, 1)"
          >下移</el-button>
          <el-button size="small" link :disabled="running" @click="duplicateStep(s.id)">复制</el-button>
          <el-button size="small" link :disabled="running" @click="removeStepAndResult(s.id)">删除</el-button>
          <el-button
            v-if="!outcomes[s.id] && !running"
            size="small"
            link
            @click="markSkipped(s)"
          >跳过</el-button>
        </div>
      </div>
      <el-tag v-if="tagText(s.id)" size="small" :type="tagType(s.id)">{{ tagText(s.id) }}</el-tag>
      <el-icon v-if="outcomes[s.id]?.status === 'ok'" class="check"><CircleCheck /></el-icon>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 单步调试「步骤列表」编辑器（从 StepSequencePanel.vue 拆出）
 *
 * 职责：渲染当前序列的步骤列表，并提供每一步的内容编辑（名称/超时/启用/HEX）
 * 与顺序维护（上移/下移/复制/删除）以及单步状态标签（成功/失败/已跳过）。
 *
 * 步骤数据来自模块级单例 useStepSequences()，与页面主组件、序列面板共用同一份状态，
 * 因此这里直接调用单例的 updateStep / moveStep / duplicateStep / removeStep 即可，
 * 不需要在父子之间传递整个序列对象。
 *
 * 与页面主组件的分工：凡是会影响「执行结果」的副作用（清掉某一步的结果、
 * 标记某一步被跳过）都通过事件交给上层处理，保证执行状态只有一处维护。
 *
 * Props:
 * - outcomes：每步执行结果（主组件维护），用于步骤状态标签与「跳过」入口显隐
 * - running：是否正在执行（执行中禁用编辑）
 * - runningStepId：当前正在执行的步骤 id
 *
 * Emits:
 * - step-content-changed：某一步内容或结构被改 → 上层只需清掉这一步的结果
 * - skip-step：用户手动跳过某一步
 *
 * 典型应用场景：报文分步调试页（StepDebug.vue）左栏「自定义序列」卡片内的步骤列表。
 */
import { computed } from 'vue'
import { CircleCheck } from '@element-plus/icons-vue'
import {
  MAX_TIMEOUT_MS,
  MIN_TIMEOUT_MS,
  useStepSequences,
  type SequenceStep,
} from '@/composables/acquisition/useStepSequences'
import type { StepOutcome } from './stepDebugTypes'

const props = defineProps<{
  /** 每步执行结果（主组件维护） */
  outcomes: Record<string, StepOutcome>
  /** 是否正在执行 */
  running: boolean
  /** 当前正在执行的步骤 id */
  runningStepId: string
}>()

const emit = defineEmits<{
  /** 某一步内容被改：上层只需清掉这一步的结果 */
  (e: 'step-content-changed', id: string): void
  /** 手动跳过某一步 */
  (e: 'skip-step', step: SequenceStep): void
}>()

// 序列状态来自模块级单例，与页面主组件共用同一份数据
const { activeSequence, updateStep, removeStep, moveStep, duplicateStep } = useStepSequences()

const pendingIndex = computed(() => activeSequence.value?.steps.findIndex((s) => !props.outcomes[s.id]) ?? -1)

function stepClass(id: string, index: number): Record<string, boolean> {
  const status = props.outcomes[id]?.status
  return {
    running: props.runningStepId === id,
    ok: status === 'ok',
    fail: status === 'fail',
    skipped: status === 'skipped',
    active: !status && pendingIndex.value === index,
  }
}

function tagText(id: string): string {
  const status = props.outcomes[id]?.status
  return status === 'ok' ? '成功' : status === 'fail' ? '失败' : status === 'skipped' ? '已跳过' : ''
}

function tagType(id: string): 'success' | 'danger' | 'info' {
  const status = props.outcomes[id]?.status
  return status === 'ok' ? 'success' : status === 'fail' ? 'danger' : 'info'
}

function onStepName(id: string, name: string): void {
  updateStep(id, { name })
  emit('step-content-changed', id)
}

function onStepHex(id: string, hex: string): void {
  updateStep(id, { hex })
  emit('step-content-changed', id)
}

function onStepTimeout(id: string, value: number | undefined): void {
  updateStep(id, { timeoutMs: value ?? 0 })
  emit('step-content-changed', id)
}

function onStepEnabled(id: string, value: string | number | boolean): void {
  updateStep(id, { enabled: value === true })
  emit('step-content-changed', id)
}

function removeStepAndResult(id: string): void {
  removeStep(id)
  emit('step-content-changed', id)
}

function markSkipped(step: SequenceStep): void {
  emit('skip-step', step)
}
</script>

<style scoped>
.step-list { max-height: 420px; overflow-y: auto; }
.step-item { display: flex; gap: 10px; padding: 10px; border: 1px solid var(--el-border-color-lighter); border-radius: var(--radius-button); margin-bottom: 8px; align-items: flex-start; }
.step-item.active { border-color: var(--el-color-primary); background: var(--el-color-primary-light-9); }
.step-item.running { border-color: var(--el-color-primary); box-shadow: 0 0 0 1px var(--el-color-primary) inset; }
.step-item.ok { opacity: 0.75; }
.step-item.fail { border-color: var(--el-color-danger); }
.step-item.skipped { opacity: 0.5; }
.step-num { width: 28px; height: 28px; border-radius: 50%; background: var(--el-color-info); color: var(--text-inverse); display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
.step-item.active .step-num, .step-item.running .step-num { background: var(--el-color-primary); }
.step-item.ok .step-num { background: var(--el-color-success); }
.step-item.fail .step-num { background: var(--el-color-danger); }
.step-body { flex: 1; min-width: 0; }
.step-row { display: flex; gap: 6px; align-items: center; margin-bottom: 6px; }
.step-name { flex: 1; }
.step-timeout { width: 110px; flex-shrink: 0; }
.step-hex { margin-bottom: 2px; }
.step-hex :deep(input) { font-family: var(--font-mono, monospace); letter-spacing: 0.5px; }
.step-tools { display: flex; gap: 8px; }
.check { color: var(--el-color-success); font-size: 1.125rem; margin-top: 4px; }
</style>
