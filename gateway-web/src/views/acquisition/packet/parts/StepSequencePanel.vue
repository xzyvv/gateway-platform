<template>
  <el-card shadow="never" class="section-card">
    <template #header>
      <div class="card-head">
        <strong>自定义序列</strong>
        <div class="head-actions">
          <el-tooltip content="撤销 (Ctrl+Z)" placement="top" :show-after="400">
            <!-- S8: 纯图标按钮只有 tooltip，读屏与键盘用户拿不到可读名称 -->
            <el-button size="small" :icon="RefreshLeft" :disabled="running || !canUndo" aria-label="撤销" title="撤销 (Ctrl+Z)" @click="onUndo" />
          </el-tooltip>
          <el-tooltip content="重做 (Ctrl+Shift+Z)" placement="top" :show-after="400">
            <el-button size="small" :icon="RefreshRight" :disabled="running || !canRedo" aria-label="重做" title="重做 (Ctrl+Shift+Z)" @click="onRedo" />
          </el-tooltip>
          <span class="action-divider" />
          <el-button size="small" :icon="Plus" :disabled="running" @click="onCreateSequence">新建</el-button>
          <el-button size="small" :icon="Delete" :disabled="running || !activeSequence" @click="onDeleteSequence">删除</el-button>
          <el-button size="small" :icon="Upload" :disabled="running" @click="pickFile">导入</el-button>
          <el-button size="small" :icon="Download" :disabled="!activeSequence" @click="exportFile">导出</el-button>
        </div>
      </div>
    </template>

    <StepSequenceToolbar
      v-model:stop-on-fail="stopOnFail"
      :bus-options="busOptions"
      :bus-loading="busLoading"
      :running="running"
      :outcomes="outcomes"
      :sequences="seqOptions"
      :active-id="activeId"
      :bus="selectedBus"
      :has-sequence="!!activeSequence"
      :seq-name="activeSequence?.name ?? ''"
      :done-count="doneCount"
      :total-count="activeSequence?.steps.length ?? 0"
      @bus-change="onBusChange"
      @select-sequence="onSelectSequence"
      @rename-sequence="onRenameSequence"
    />

    <StepListEditor
      :outcomes="outcomes"
      :running="running"
      :running-step-id="runningStepId"
      @step-content-changed="emit('step-content-changed', $event)"
      @skip-step="emit('skip-step', $event)"
    />

    <div class="actions">
      <el-button type="primary" :icon="VideoPlay" :disabled="running || !hasPending" @click="emit('step-once')">单步执行</el-button>
      <el-button :disabled="running || !hasPending" @click="emit('run-rest')">自动执行剩余</el-button>
      <el-button v-if="running" type="danger" plain @click="emit('abort')">中止</el-button>
      <el-button :icon="RefreshLeft" :disabled="running || doneCount === 0" @click="emit('reset-progress')">重置进度</el-button>
    </div>
    <div class="actions">
      <el-button :icon="Plus" :disabled="running" @click="addBlankStep">添加步骤</el-button>
      <el-dropdown @command="onAddPreset">
        <el-button :disabled="running">
          从预置添加<el-icon class="el-icon--right"><ArrowDown /></el-icon>
        </el-button>
        <template #dropdown>
          <el-dropdown-menu>
            <el-dropdown-item v-for="p in presets" :key="p.hex" :command="p">{{ p.name }}</el-dropdown-item>
          </el-dropdown-menu>
        </template>
      </el-dropdown>
    </div>
    <div class="hint">序列仅保存在本机浏览器；导出 JSON 可交接给同事或换机使用。</div>
    <input ref="fileInput" type="file" accept=".json,application/json" class="file-input" @change="onFileChange" />
  </el-card>
</template>

<script setup lang="ts">
/**
 * 单步调试「自定义序列」编辑面板（从 StepDebug.vue 拆出）
 *
 * 职责：序列的增删改查（新建/删除/导入/导出/重命名）、步骤的增删（添加/从预置添加）、
 * 以及执行入口（单步执行 / 自动执行剩余 / 中止 / 重置进度）。
 *
 * 与页面主组件的分工：本面板只改「序列」，凡是会影响「执行结果」的副作用
 * （清空进度、标记中止、跳过一个步骤）都通过事件交给主组件执行，
 * 保证执行状态只有一处维护。
 *
 * 展示部分继续按区块下钻（parts/ 下）：
 * - parts/StepSequenceToolbar.vue 总线/序列选择、进度、失败即停开关
 * - parts/StepListEditor.vue      步骤列表与每步的内容编辑
 *
 * Props:
 * - busOptions：总线选项（来自 GET /api/config 的 buses）
 * - busLoading：总线选项加载中
 * - outcomes：每步执行结果（主组件维护），用于步骤状态标签与进度
 * - running：是否正在执行（执行中禁用编辑）
 * - runningStepId：当前正在执行的步骤 id
 * - presets：后端预置帧（供「从预置添加」）
 *
 * Emits:
 * - structure-changed：序列结构变化（新建/删除/切换/导入/换总线/撤销重做）→ 主组件清空执行进度
 * - step-content-changed：某一步内容被改 → 主组件只清掉这一步的结果
 * - skip-step：用户手动跳过某一步
 * - abort：请求中止正在执行的批次
 * - step-once / run-rest：单步执行 / 自动执行剩余
 * - reset-progress：重置执行进度
 *
 * 典型应用场景：报文分步调试页（StepDebug.vue）左栏的「自定义序列」卡片。
 */
import { computed, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { ArrowDown, Delete, Download, Plus, RefreshLeft, RefreshRight, Upload, VideoPlay } from '@element-plus/icons-vue'
import { useSharedDebugTarget } from '@/composables/acquisition/useDebugTargets'
import {
  useStepSequences,
  type SequenceStep,
} from '@/composables/acquisition/useStepSequences'
import { confirmDelete } from '@/utils/confirm'
import { triggerDownload } from '@/utils/exportCsv'
import type { PacketPreset } from '@/api/protocolTools'
import StepSequenceToolbar from './StepSequenceToolbar.vue'
import StepListEditor from './StepListEditor.vue'
import type { StepOutcome } from './stepDebugTypes'

const props = defineProps<{
  /** 总线选项 */
  busOptions: Array<{ label: string; value: string }>
  /** 总线选项加载中 */
  busLoading: boolean
  /** 每步执行结果（主组件维护） */
  outcomes: Record<string, StepOutcome>
  /** 是否正在执行 */
  running: boolean
  /** 当前正在执行的步骤 id */
  runningStepId: string
  /** 后端预置帧 */
  presets: PacketPreset[]
}>()

const emit = defineEmits<{
  /** 序列结构变化：主组件需清空执行进度 */
  (e: 'structure-changed'): void
  /** 某一步内容被改：主组件只需清掉这一步的结果 */
  (e: 'step-content-changed', id: string): void
  /** 手动跳过某一步 */
  (e: 'skip-step', step: SequenceStep): void
  /** 请求中止正在执行的批次 */
  (e: 'abort'): void
  /** 单步执行 */
  (e: 'step-once'): void
  /** 自动执行剩余步骤 */
  (e: 'run-rest'): void
  /** 重置执行进度 */
  (e: 'reset-progress'): void
}>()

/** 失败即停开关（与主组件双向绑定） */
const stopOnFail = defineModel<boolean>('stopOnFail', { required: true })

// 序列状态来自模块级单例，与页面主组件共用同一份数据
const {
  sequences,
  activeId,
  activeSequence,
  canUndo,
  canRedo,
  undo,
  redo,
  selectSequence,
  createSequence,
  deleteSequence,
  renameSequence,
  setSequenceBus,
  addStep,
  toJson,
  importFromJson,
} = useStepSequences()
// 共享调试目标里的总线（与页面主组件同一个 ref）
const { selectedBus } = useSharedDebugTarget()

const fileInput = ref<HTMLInputElement | null>(null)

/** 序列下拉选项 */
const seqOptions = computed(() => sequences.value.map((s) => ({ id: s.id, name: s.name })))
const pendingIndex = computed(() => activeSequence.value?.steps.findIndex((s) => !props.outcomes[s.id]) ?? -1)
const hasPending = computed(() => pendingIndex.value >= 0)
const doneCount = computed(() => activeSequence.value?.steps.filter((s) => props.outcomes[s.id]).length ?? 0)

/** 序列自带的 bus 优先（导出后换网关也能复用），否则把当前总线记进序列 */
function applySequenceBus(): void {
  const seq = activeSequence.value
  if (!seq) return
  if (seq.bus && props.busOptions.some((o) => o.value === seq.bus)) {
    if (selectedBus.value !== seq.bus) selectedBus.value = seq.bus
  } else if (selectedBus.value) {
    setSequenceBus(seq.id, selectedBus.value)
  }
}

function onBusChange(bus: string): void {
  selectedBus.value = bus
  if (activeSequence.value) setSequenceBus(activeSequence.value.id, bus)
  // 换总线后旧结果不再对应当前目标：清进度，日志保留便于对比
  emit('structure-changed')
}

function onCreateSequence(): void {
  createSequence('新序列', selectedBus.value)
  emit('structure-changed')
  emit('abort')
}

async function onDeleteSequence(): Promise<void> {
  const seq = activeSequence.value
  if (!seq) return
  if (!(await confirmDelete(seq.name, { title: '删除序列' }))) return
  deleteSequence(seq.id)
  emit('structure-changed')
  emit('abort')
  applySequenceBus()
  ElMessage.success('已删除')
}

function onSelectSequence(id: string): void {
  selectSequence(id)
  emit('structure-changed')
  emit('abort')
  applySequenceBus()
}

function onRenameSequence(name: string): void {
  const seq = activeSequence.value
  if (seq) renameSequence(seq.id, name)
}

function addBlankStep(): void {
  if (!addStep()) ElMessage.warning('步骤已达上限')
}

function onAddPreset(preset: { name: string; hex: string }): void {
  addStep({ name: preset.name, hex: preset.hex })
}

function pickFile(): void {
  fileInput.value?.click()
}

async function onFileChange(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  // 清空 value 允许重复选择同一个文件
  input.value = ''
  if (!file) return
  if (file.size > 1024 * 1024) {
    ElMessage.error('文件过大（上限 1MB）')
    return
  }
  const result = importFromJson(await file.text())
  if (!result.ok) {
    ElMessage.error(result.message)
    return
  }
  emit('structure-changed')
  emit('abort')
  applySequenceBus()
  ElMessage.success(result.message)
}

function safeFileName(name: string): string {
  return name.replace(/[\\/:*?"<>|\s]+/g, '_').slice(0, 60) || 'step-sequence'
}

function exportFile(): void {
  const seq = activeSequence.value
  if (!seq) return
  const blob = new Blob([toJson()], { type: 'application/json;charset=utf-8' })
  triggerDownload(blob, `${safeFileName(seq.name)}.json`)
}

/** 撤销/重做：结构变更（增删步/序列、移动、导入）才入栈；步内容编辑靠输入框原生 Ctrl+Z。 */
function onUndo(): void {
  if (props.running || !canUndo.value) return
  undo()
  emit('structure-changed')
}

function onRedo(): void {
  if (props.running || !canRedo.value) return
  redo()
  emit('structure-changed')
}
</script>

<style scoped src="./stepDebugShared.css"></style>

<style scoped>
.action-divider { width: 1px; height: 18px; background: var(--el-border-color); margin: 0 4px; }
.actions { margin-top: 10px; display: flex; gap: 8px; flex-wrap: wrap; }
.hint { margin-top: 10px; font-size: 0.75rem; color: var(--el-text-color-secondary); }
.file-input { display: none; }
</style>
