/**
 * 单步调试「自定义序列」本地存储（A 档：纯前端，零后端改动）
 *
 * 序列只保存在本机浏览器（按激活网关命名空间隔离，见 gatewayStorage），
 * 不落网关、不跨人共享；导出 JSON 是唯一的交接方式。
 *
 * 执行不走本模块：每一步仍调用与报文编辑器相同的真实下发接口
 * `POST /api/protocol-tools/packet-editor/send`（每次独立抢总线租约），
 * 因此「步与步之间」采集仍可插入——需要整段独占总线的时序敏感序列不属于 A 档。
 *
 * 为什么用模块级 ref：切换到其它 tab 再回来时序列与选择需要保持（等价
 * useFavorites 的 @singleton 约定），且避免多实例各自挂 watch 重复写盘。
 */
import { computed, ref, watch } from 'vue'
import { scopedGet, scopedRemove, scopedSet } from '@/utils/gatewayStorage'
import { isValidHex, normalizeHex } from '@/utils/hexFrame'

// 帧文本规范/合法性判断已移到 utils/hexFrame（报文编辑也要用），这里保留再导出兼容既有引用
export { isValidHex, normalizeHex }

export interface SequenceStep {
  /** 稳定 key（用于执行结果映射与列表渲染） */
  id: string
  /** 步骤名（可空，仅作展示） */
  name: string
  /** 原始 HEX（允许空格；发送前统一 normalizeHex） */
  hex: string
  /** 单帧等待超时（ms），100–10000 */
  timeoutMs: number
  /** 关闭后自动执行会跳过该步（单步执行也跳过） */
  enabled: boolean
}

export interface StepSequence {
  id: string
  name: string
  /** 所属总线 port_name；导出后可在别的网关按同名总线复用 */
  bus: string
  steps: SequenceStep[]
  updatedAt: number
}

const STORAGE_KEY = 'gw_step_debug_sequences'
const ACTIVE_KEY = 'gw_step_debug_active'
/** 防御脏 localStorage：单文件最多承载的序列数 */
const MAX_SEQUENCES = 200
const MAX_STEPS = 200

export const DEFAULT_TIMEOUT_MS = 1500
export const MIN_TIMEOUT_MS = 100
export const MAX_TIMEOUT_MS = 10_000

function uid(prefix: string): string {
  const rnd = typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`
  return `${prefix}-${rnd}`
}

/** 超时收窄到 [100, 10000]，非数值回落默认值 */
export function clampTimeout(ms: number): number {
  if (!Number.isFinite(ms)) return DEFAULT_TIMEOUT_MS
  return Math.min(MAX_TIMEOUT_MS, Math.max(MIN_TIMEOUT_MS, Math.round(ms)))
}

export function makeStep(patch: Partial<SequenceStep> = {}): SequenceStep {
  return {
    id: patch.id ?? uid('step'),
    name: patch.name ?? '',
    hex: normalizeHex(patch.hex ?? ''),
    timeoutMs: clampTimeout(patch.timeoutMs ?? DEFAULT_TIMEOUT_MS),
    enabled: patch.enabled ?? true,
  }
}

/** 预置帧形状（与后端 `listPresets()` 返回一致） */
export interface PresetFrame {
  name: string
  hex: string
}

/** 首次进入的起点：步骤由后端预置帧补齐（见 seedDefaultSequence） */
export function makeDefaultSequence(bus = '', presets: readonly PresetFrame[] = []): StepSequence {
  return {
    id: uid('seq'),
    name: '默认序列',
    bus,
    steps: presets.map((p) => makeStep({ name: p.name, hex: p.hex })),
    updatedAt: Date.now(),
  }
}

function sanitizeStep(raw: unknown, used: Set<string>): SequenceStep | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const hex = typeof r.hex === 'string' ? normalizeHex(r.hex) : ''
  // 非法 HEX 的步骤无法执行，导入时直接丢弃
  if (!isValidHex(hex)) return null
  let id = typeof r.id === 'string' && r.id ? r.id : uid('step')
  if (used.has(id)) id = uid('step')
  used.add(id)
  return makeStep({
    id,
    name: typeof r.name === 'string' ? r.name : '',
    hex,
    timeoutMs: typeof r.timeoutMs === 'number' ? r.timeoutMs : DEFAULT_TIMEOUT_MS,
    enabled: r.enabled !== false,
  })
}

function sanitizeSequence(raw: unknown): StepSequence | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const rawSteps = Array.isArray(r.steps) ? r.steps.slice(0, MAX_STEPS) : []
  const used = new Set<string>()
  const steps: SequenceStep[] = []
  for (const item of rawSteps) {
    const s = sanitizeStep(item, used)
    if (s) steps.push(s)
  }
  return {
    id: typeof r.id === 'string' && r.id ? r.id : uid('seq'),
    name: typeof r.name === 'string' && r.name.trim() ? r.name.trim() : '未命名序列',
    bus: typeof r.bus === 'string' ? r.bus : '',
    steps,
    updatedAt: typeof r.updatedAt === 'number' ? r.updatedAt : Date.now(),
  }
}

function readSequences(): StepSequence[] {
  try {
    const raw = scopedGet(STORAGE_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    const list = Array.isArray(parsed) ? parsed.slice(0, MAX_SEQUENCES) : []
    const used = new Set<string>()
    const out: StepSequence[] = []
    for (const item of list) {
      const seq = sanitizeSequence(item)
      if (!seq) continue
      if (used.has(seq.id)) seq.id = uid('seq')
      used.add(seq.id)
      out.push(seq)
    }
    return out
  } catch {
    return []
  }
}

const initial = readSequences()
/** 首次自动创建默认序列（本地无数据时）——预置帧加载后可补齐步骤 */
const autoCreatedDefault = initial.length === 0
// @singleton: 序列与当前选择需跨 tab 切换保持，且避免多实例重复写盘（同 useFavorites 约定）
// eslint-disable-next-line no-restricted-syntax
const sequences = ref<StepSequence[]>(initial.length ? initial : [makeDefaultSequence()])
const storedActive = scopedGet(ACTIVE_KEY) ?? ''
// eslint-disable-next-line no-restricted-syntax
const activeId = ref(sequences.value.some((s) => s.id === storedActive) ? storedActive : sequences.value[0]!.id)

function persist(): void {
  try {
    scopedSet(STORAGE_KEY, JSON.stringify(sequences.value))
  } catch {
    // localStorage 满/被禁用：静默，内存态仍可用，用户可通过导出保留序列
  }
}

watch(sequences, persist, { deep: true })
watch(activeId, (v) => {
  try {
    if (v) scopedSet(ACTIVE_KEY, v)
    else scopedRemove(ACTIVE_KEY)
  } catch {
    // ignore
  }
})
// 首次进入（含自动补默认序列）立即落盘，避免「一个有内容的默认序列却从未保存」
persist()

/**
 * 后端预置帧加载完成后调用：把「首次自动创建的默认序列」补成预置步骤。
 *
 * 仅在用户尚未编辑（仍是唯一一个空序列）时生效，不会覆盖已有序列。
 */
export function seedDefaultSequence(presets: readonly PresetFrame[]): void {
  if (!autoCreatedDefault || presets.length === 0) return
  const seq = sequences.value[0]
  if (sequences.value.length !== 1 || !seq || seq.steps.length > 0) return
  seq.steps = presets.map((p) => makeStep({ name: p.name, hex: p.hex }))
  seq.updatedAt = Date.now()
}

/* ========== undo / redo ==========
 * 调试序列编辑时误删一步/一个序列代价很高（HEX 与超时都得重填）。
 * 用「全量快照栈」实现：每次结构变更前把 sequences 深拷贝压入 past，
 * undo 时把当前态压入 future 并恢复 past 栈顶。
 *
 * 只在结构变更时入栈（create/delete/rename/bus/add/remove/move/duplicate/import），
 * updateStep（逐键输入名称/HEX/超时）不入栈——否则每按一键就产生一条历史，
 * 栈很快被无意义的中间态填满，真正想撤销的「删除」反而被挤出。
 * 步骤内容编辑如需撤销，靠浏览器输入框自身的 Ctrl+Z（原生行为）兜底。
 */
const MAX_HISTORY = 50
// 用 ref 包裹：canUndo/canRedo 是 computed，需要响应式追踪 past/future 的长度变化。
// 用 ref 而非 reactive 数组，因为我们只在结构变更时整体替换/截断，不需要 deep 监听。
// eslint-disable-next-line no-restricted-syntax
const past = ref<StepSequence[][]>([])
// eslint-disable-next-line no-restricted-syntax
const future = ref<StepSequence[][]>([])

function cloneSequences(): StepSequence[] {
  return JSON.parse(JSON.stringify(sequences.value)) as StepSequence[]
}

/** 结构变更前调用：把当前态压入 past，清空 future（新分支后 redo 无意义） */
function commit(): void {
  past.value.push(cloneSequences())
  if (past.value.length > MAX_HISTORY) past.value.shift()
  future.value = []
}

function restore(snapshot: StepSequence[]): void {
  sequences.value = snapshot
  // 恢复后 activeId 可能指向已不存在的序列，夹紧到首个
  if (!sequences.value.some((s) => s.id === activeId.value)) {
    activeId.value = sequences.value[0]?.id ?? ''
  }
}

export function useStepSequences() {
  const activeSequence = computed(() => sequences.value.find((s) => s.id === activeId.value) ?? null)
  const canUndo = computed(() => past.value.length > 0)
  const canRedo = computed(() => future.value.length > 0)

  function find(id: string): StepSequence | undefined {
    return sequences.value.find((s) => s.id === id)
  }

  function touch(seq: StepSequence): void {
    seq.updatedAt = Date.now()
  }

  function selectSequence(id: string): void {
    if (find(id)) activeId.value = id
  }

  function undo(): void {
    const prev = past.value.pop()
    if (!prev) return
    future.value.push(cloneSequences())
    restore(prev)
  }

  function redo(): void {
    const next = future.value.pop()
    if (!next) return
    past.value.push(cloneSequences())
    restore(next)
  }

  function createSequence(name = '新序列', bus = ''): StepSequence {
    commit()
    const seq: StepSequence = { id: uid('seq'), name: name.trim() || '新序列', bus, steps: [], updatedAt: Date.now() }
    sequences.value.push(seq)
    activeId.value = seq.id
    return seq
  }

  /** 删除序列；最后一个删掉后自动补一个空的默认序列，避免页面无可用对象 */
  function deleteSequence(id: string): void {
    if (!find(id)) return
    commit()
    if (sequences.value.length <= 1) {
      const seq = { ...makeDefaultSequence(), steps: [] }
      sequences.value = [seq]
      activeId.value = seq.id
      return
    }
    sequences.value = sequences.value.filter((s) => s.id !== id)
    if (activeId.value === id) activeId.value = sequences.value[0]!.id
  }

  function renameSequence(id: string, name: string): void {
    const seq = find(id)
    if (!seq) return
    commit()
    seq.name = name.trim() || '未命名序列'
    touch(seq)
  }

  function setSequenceBus(id: string, bus: string): void {
    const seq = find(id)
    if (!seq || seq.bus === bus) return
    commit()
    seq.bus = bus
    touch(seq)
  }

  function addStep(patch: Partial<SequenceStep> = {}): SequenceStep | null {
    const seq = activeSequence.value
    if (!seq || seq.steps.length >= MAX_STEPS) return null
    commit()
    const step = makeStep(patch)
    seq.steps.push(step)
    touch(seq)
    return step
  }

  function updateStep(id: string, patch: Partial<Omit<SequenceStep, 'id'>>): void {
    const seq = activeSequence.value
    const step = seq?.steps.find((s) => s.id === id)
    if (!seq || !step) return
    if (patch.name !== undefined) step.name = patch.name
    if (patch.hex !== undefined) step.hex = patch.hex
    if (patch.timeoutMs !== undefined) step.timeoutMs = clampTimeout(patch.timeoutMs)
    if (patch.enabled !== undefined) step.enabled = patch.enabled
    touch(seq)
  }

  function removeStep(id: string): void {
    const seq = activeSequence.value
    if (!seq) return
    const idx = seq.steps.findIndex((s) => s.id === id)
    if (idx < 0) return
    commit()
    seq.steps.splice(idx, 1)
    touch(seq)
  }

  /** 上移/下移一步（delta = -1 / 1），越界自动夹紧 */
  function moveStep(id: string, delta: number): void {
    const seq = activeSequence.value
    if (!seq) return
    const idx = seq.steps.findIndex((s) => s.id === id)
    if (idx < 0) return
    const next = Math.min(seq.steps.length - 1, Math.max(0, idx + delta))
    if (next === idx) return
    commit()
    const [step] = seq.steps.splice(idx, 1)
    if (!step) return
    seq.steps.splice(next, 0, step)
    touch(seq)
  }

  function duplicateStep(id: string): SequenceStep | null {
    const seq = activeSequence.value
    if (!seq) return null
    const idx = seq.steps.findIndex((s) => s.id === id)
    if (idx < 0 || seq.steps.length >= MAX_STEPS) return null
    commit()
    const src = seq.steps[idx]!
    const copy = makeStep({ name: src.name ? `${src.name} 副本` : '', hex: src.hex, timeoutMs: src.timeoutMs, enabled: src.enabled })
    seq.steps.splice(idx + 1, 0, copy)
    touch(seq)
    return copy
  }

  /** 当前序列导出为 JSON（供交接/备份） */
  function toJson(): string {
    return JSON.stringify(activeSequence.value ?? { steps: [] }, null, 2)
  }

  /**
   * 导入 JSON：支持单个序列对象或序列数组，追加到现有列表（不覆盖）。
   * 非法步骤（HEX 不合法）会被跳过；无法解析或没有有效序列时返回失败原因。
   */
  function importFromJson(text: string): { ok: boolean; message: string } {
    let parsed: unknown
    try {
      parsed = JSON.parse(text)
    } catch {
      return { ok: false, message: 'JSON 解析失败，请确认文件内容' }
    }
    const items = Array.isArray(parsed) ? parsed.slice(0, MAX_SEQUENCES) : [parsed]
    const imported = items.map(sanitizeSequence).filter((s): s is StepSequence => s !== null)
    if (!imported.length) {
      return { ok: false, message: '未找到有效序列（需要含合法 HEX 的 steps 数组）' }
    }
    commit()
    const existing = new Set(sequences.value.map((s) => s.id))
    for (const seq of imported) {
      if (existing.has(seq.id)) seq.id = uid('seq')
      existing.add(seq.id)
      seq.updatedAt = Date.now()
      sequences.value.push(seq)
    }
    activeId.value = imported[imported.length - 1]!.id
    return { ok: true, message: `已导入 ${imported.length} 个序列` }
  }

  return {
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
    updateStep,
    removeStep,
    moveStep,
    duplicateStep,
    toJson,
    importFromJson,
  }
}
