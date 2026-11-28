<template>
  <div class="sd">
    <SectionHeader title="单步调试" icon="VideoPlay" />

    <el-row :gutter="16">
      <el-col :span="11">
        <StepSequencePanel
          v-model:stop-on-fail="stopOnFail"
          :bus-options="busOptions"
          :bus-loading="loading"
          :outcomes="outcomes"
          :running="running"
          :running-step-id="runningStepId"
          :presets="presets"
          @structure-changed="clearOutcomes"
          @step-content-changed="clearOutcome"
          @skip-step="markSkipped"
          @abort="onAbort"
          @step-once="stepOnce"
          @run-rest="runRest"
          @reset-progress="resetProgress"
        />
      </el-col>

      <el-col :span="13">
        <StepRunLogPanel
          :logs="logs"
          @copy="copyLog"
          @copy-all="copyAllLogs"
          @export="exportLogs"
          @clear="clearLogs"
        />
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
/**
 * 单步调试（A 档：自定义序列，纯前端，零后端改动）
 *
 * 与旧版的区别：
 * - 序列可编辑/可保存/可导出（localStorage，按网关隔离），不再是后端写死的 5 帧
 * - 每一步都走真实的 `POST /packet-editor/send`，看到真实请求/响应/耗时
 * - 状态只存本页，不再依赖后端全局 static，多用户不会互相覆盖日志
 *
 * 已知边界：每步独立抢总线租约，步与步之间采集可插入，因此时序敏感序列
 * （如「唤醒后 100ms 内读」）不属于本档能力。
 *
 * 本文件只保留「执行引擎 + 页面编排」，展示部分按区块拆到同目录 components/ 下：
 * - components/StepSequencePanel.vue 序列编辑区（序列/步骤编辑、导入导出、撤销重做）
 * - components/StepRunLogPanel.vue   执行日志区
 * - components/stepDebugTypes.ts     共享类型与错误码文案
 */
import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { listPresets, sendFrame, type PacketPreset } from '@/api/protocolTools'
import { abortAllInflight } from '@/api/http'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import {
  clampTimeout,
  isValidHex,
  normalizeHex,
  seedDefaultSequence,
  useStepSequences,
  type SequenceStep,
} from '@/composables/acquisition/useStepSequences'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { isWriteFrame } from '@/utils/hex/hexFrame'
import { copyText } from '@/utils/ui/clipboard'
import { csvFilename, downloadTableCsv } from '@/utils/csv/exportCsv'
import { confirmAction } from '@/utils/ui/confirm'
import StepSequencePanel from './components/StepSequencePanel.vue'
import StepRunLogPanel from './components/StepRunLogPanel.vue'
import { errorLabel, type RunLog, type StepOutcome } from './components/stepDebugTypes'

const { selectedBus, busOptions, loading } = useDebugTargets()
const { showToast, message } = useErrorDisplay()
const { activeSequence, canUndo, canRedo, undo, redo } = useStepSequences()

/** 预置帧：后端唯一定义（P3-4），供「从预置添加」下拉使用 */
const presets = ref<PacketPreset[]>([])
/** 每步执行结果：驱动左侧状态标签与进度 */
const outcomes = ref<Record<string, StepOutcome>>({})
/** 执行日志：追加式，重置进度不清空，便于保留取证记录 */
const logs = ref<RunLog[]>([])
const runningStepId = ref('')
const running = ref(false)
const abortRequested = ref(false)
const stopOnFail = ref(true)
let logSeq = 0

const pendingIndex = computed(() => activeSequence.value?.steps.findIndex((s) => !outcomes.value[s.id]) ?? -1)

function clearOutcomes(): void {
  outcomes.value = {}
}

function clearOutcome(id: string): void {
  if (outcomes.value[id]) delete outcomes.value[id]
}

function markSkipped(step: SequenceStep): void {
  outcomes.value[step.id] = {
    status: 'skipped',
    requestHex: normalizeHex(step.hex),
    responseHex: '',
    parsed: '',
    latencyMs: 0,
    error: null,
    at: Date.now(),
  }
}

function appendLog(step: SequenceStep, index: number, outcome: StepOutcome): void {
  logs.value.unshift({ ...outcome, key: `${step.id}-${outcome.at}-${logSeq++}`, step: index + 1, bus: selectedBus.value })
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** 执行单步：真实下发，结果同时写入步骤状态与日志 */
async function runStep(step: SequenceStep, index: number): Promise<boolean> {
  if (!selectedBus.value) {
    ElMessage.warning('请先选择总线')
    return false
  }
  const hex = normalizeHex(step.hex)
  if (!isValidHex(hex)) {
    ElMessage.error(`第 ${index + 1} 步 HEX 非法（需偶数个十六进制字符）`)
    return false
  }
  runningStepId.value = step.id
  try {
    const r = await sendFrame(hex, selectedBus.value, clampTimeout(step.timeoutMs))
    const outcome: StepOutcome = {
      status: r.success ? 'ok' : 'fail',
      requestHex: r.requestHex,
      responseHex: r.responseHex,
      parsed: r.parsed,
      latencyMs: r.latencyMs,
      error: r.error ?? null,
      at: Date.now(),
    }
    outcomes.value[step.id] = outcome
    appendLog(step, index, outcome)
    if (!r.success) ElMessage.warning(`第 ${index + 1} 步失败：${errorLabel(r.error) || '未知错误'}`)
    return r.success
  } catch (e) {
    const outcome: StepOutcome = {
      status: 'fail',
      requestHex: hex,
      responseHex: '',
      parsed: '',
      latencyMs: 0,
      error: message(e, '发送失败'),
      at: Date.now(),
    }
    outcomes.value[step.id] = outcome
    appendLog(step, index, outcome)
    showToast(e, `第 ${index + 1} 步发送失败`)
    return false
  } finally {
    runningStepId.value = ''
  }
}

async function stepOnce(): Promise<void> {
  const seq = activeSequence.value
  if (!seq || running.value) return
  if (!selectedBus.value) {
    ElMessage.warning('请先选择总线')
    return
  }
  // 禁用的步骤不发送，直接标记跳过（否则会卡在它上面无法继续）
  let i = pendingIndex.value
  while (i >= 0) {
    const step = seq.steps[i]
    if (!step) return
    if (step.enabled) {
      await runStep(step, i)
      return
    }
    markSkipped(step)
    i = pendingIndex.value
  }
}

function pendingSteps(): Array<{ step: SequenceStep; index: number }> {
  const seq = activeSequence.value
  if (!seq) return []
  return seq.steps
    .map((step, index) => ({ step, index }))
    .filter(({ step }) => !outcomes.value[step.id])
}

async function runRest(): Promise<void> {
  const seq = activeSequence.value
  if (!seq || running.value) return
  if (!selectedBus.value) {
    ElMessage.warning('请先选择总线')
    return
  }
  const pending = pendingSteps()
  if (pending.length === 0) {
    ElMessage.info('没有待执行步骤')
    return
  }
  const writes = pending.filter(({ step }) => step.enabled && isWriteFrame(step.hex)).length
  const ok = await confirmAction({
    message: `将依次向 ${selectedBus.value} 下发剩余 ${pending.length} 步${writes ? `（其中 ${writes} 步为写操作）` : ''}。执行期间可点「中止」，但已发出的帧不可撤回。`,
    title: '自动执行剩余',
    confirmText: '开始执行',
  })
  if (!ok) return

  running.value = true
  abortRequested.value = false
  try {
    for (const { step, index } of pending) {
      if (abortRequested.value) break
      if (!step.enabled) {
        markSkipped(step)
        continue
      }
      const okStep = await runStep(step, index)
      if (abortRequested.value) break
      if (!okStep && stopOnFail.value) {
        ElMessage.warning(`第 ${index + 1} 步失败，已按「失败即停」停止`)
        break
      }
      await sleep(120)
    }
  } finally {
    running.value = false
    if (abortRequested.value) ElMessage.info('已中止，未执行的步骤保留为待执行')
  }
}

function resetProgress(): void {
  abortRequested.value = false
  clearOutcomes()
  ElMessage.success('已重置进度（日志保留）')
}

/** 中止：已在飞行中的帧不可撤回，只影响后续步骤 */
function onAbort(): void {
  abortRequested.value = true
}

function logToText(l: RunLog): string {
  const lines = [`步骤 ${l.step} ${l.status === 'ok' ? '成功' : '失败'}（总线 ${l.bus || '-'}）`, `请求: ${l.requestHex}`]
  if (l.responseHex) lines.push(`响应: ${l.responseHex}`)
  if (l.parsed) lines.push(`值: ${l.parsed}`)
  if (l.latencyMs) lines.push(`耗时: ${l.latencyMs}ms`)
  if (l.error) lines.push(`错误: ${errorLabel(l.error)}`)
  return lines.join('\n')
}

function copyLog(l: RunLog): void {
  void copyText(logToText(l), `步骤 ${l.step} 已复制`)
}

function copyAllLogs(): void {
  void copyText(logs.value.map(logToText).join('\n\n'), `已复制 ${logs.value.length} 条日志`)
}

/** 导出执行日志为 CSV：现场要把 20 步的耗时/失败点发给研发，逐条复制不现实 */
function exportLogs(): void {
  if (!logs.value.length) return
  // logs 是倒序（最新在前），导出时翻回执行顺序，与页面阅读顺序一致
  downloadTableCsv(
    ['步骤', '结果', '总线', '请求HEX', '响应HEX', '解析值', '耗时ms', '错误'],
    [...logs.value].reverse().map((l) => [
      l.step,
      l.status === 'ok' ? '成功' : '失败',
      l.bus || '',
      l.requestHex,
      l.responseHex,
      l.parsed,
      l.latencyMs,
      l.error ? errorLabel(l.error) : '',
    ]),
    csvFilename('step-debug-log'),
  )
  ElMessage.success(`已导出 ${logs.value.length} 条执行日志`)
}

function clearLogs(): void {
  logs.value = []
}

/** 撤销/重做：结构变更（增删步/序列、移动、导入）才入栈；步内容编辑靠输入框原生 Ctrl+Z。 */
function onUndo(): void {
  if (running.value || !canUndo.value) return
  undo()
  clearOutcomes()
}

function onRedo(): void {
  if (running.value || !canRedo.value) return
  redo()
  clearOutcomes()
}

/** 与 Layout.vue 相同的输入态判定：输入框内不拦截 Ctrl+Z，让原生文本撤销生效。 */
function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  if (!el) return false
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable
}

function onKeydown(e: KeyboardEvent): void {
  if (e.ctrlKey || e.metaKey) {
    // Ctrl+Z 撤销；Ctrl+Shift+Z 或 Ctrl+Y 重做
    if (e.key === 'z' || e.key === 'Z') {
      if (isTypingTarget(e.target)) return
      e.preventDefault()
      if (e.shiftKey) onRedo()
      else onUndo()
      return
    }
    if (e.key === 'y' || e.key === 'Y') {
      if (isTypingTarget(e.target)) return
      e.preventDefault()
      onRedo()
    }
  }
}

/**
 * 全局 Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y 监听的挂载与摘除。
 *
 * 为什么需要显式配对（而不是只在 onMounted/onUnmounted 里加减）：
 * 本页在 ProtocolConsoleHub 的 `<KeepAlive>` 内，切页签只触发 `onDeactivated`、
 * **不触发 `onUnmounted`**。原先只在 onUnmounted 摘监听，切到「报文捕获」后这个
 * 全局 keydown 依然挂在 window 上——用户在那页按 Ctrl+Z，命中的是本页的撤销逻辑，
 * 静默改坏一条他根本没在看的报文序列。
 *
 * 用标志位而非直接调用 add/remove：非 KeepAlive 场景下 onActivated 不触发，
 * 而 KeepAlive 场景下 onMounted 与 onActivated 都会触发一次，需要幂等。
 */
let keydownAttached = false
function attachKeydown(): void {
  if (keydownAttached) return
  window.addEventListener('keydown', onKeydown)
  keydownAttached = true
}
function detachKeydown(): void {
  if (!keydownAttached) return
  window.removeEventListener('keydown', onKeydown)
  keydownAttached = false
}

onMounted(async () => {
  attachKeydown()
  // 预置帧来自后端（P3-4）；首次进入的默认序列据此补齐，避免前后端两份常量漂移
  const frames: PacketPreset[] = await listPresets().catch(() => [])
  presets.value = frames
  seedDefaultSequence(frames)
})

/** 切回页签：恢复全局快捷键（非 KeepAlive 场景不触发，onMounted 已挂好） */
onActivated(attachKeydown)

/**
 * 切走页签：摘监听 + 停止自动执行。
 *
 * 停止执行而不是「取消在飞请求」：`abortAllInflight` 是全局核选项（切网关/登出用），
 * 在这里调会把其它页签正在跑的请求一起打断。而本页的自动执行循环是逐个 await 下发，
 * 把 `abortRequested` 置位即可在当前步返回后跳出循环；已发出的帧本就不能撤回。
 * 不这样做的话，用户切到别的页签后本页仍会持续向真实设备下帧，现场只看到设备莫名被写、
 * 且界面上没有任何「仍在执行」的提示，也就无从点「中止」。
 */
onDeactivated(() => {
  detachKeydown()
  abortRequested.value = true
})

onUnmounted(() => {
  detachKeydown()
  abortRequested.value = true
  abortAllInflight('StepDebug unmount: abort inflight step-debug requests')
})
</script>

<style scoped>
.sd { padding: 0px; }
</style>
