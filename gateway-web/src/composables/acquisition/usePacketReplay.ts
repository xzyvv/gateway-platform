/**
 * 报文回放页状态机（从 PacketReplay.vue 拆出）
 *
 * 负责：回放文件列表/会话加载、预置会话保护、真实回放（默认 P0-1b 流式，
 * 旧浏览器 / Tauri 回落 P0-1 阻塞）、逐帧中间态与汇总、停止/重置/另存。
 * 页面组件只保留布局与事件接线，逐帧列表与运行状态条在 packet/components/ 下渲染。
 *
 * 为什么状态集中在这里：回放是「一条写链路 + 一份执行结果」——列表选中、
 * 进度条、逐帧状态、停止按钮都读同一份状态，父子各维护一份必然串台。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { ElMessageBox } from 'element-plus'
import {
  listSessions,
  getSession,
  executeReplayBlocking,
  executeReplayStream,
  canStreamReplay,
  saveSession,
  deleteSession,
  type ReplaySession,
  type ReplayExecuteResult,
  type ReplayFrameResult,
} from '@/api/protocolTools'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { toTaskTerminal, type TaskTerminal } from '@/composables/core/useTaskProgress'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import { useRequestGuard } from '@/composables/core/useRequestGuard'
import { confirmDelete } from '@/utils/confirm'
import { statusTagType as baseStatusTagType, type TagType } from '@/utils/statusType'

/** 预置会话不可删（后端也拒绝，前端灰色 + tooltip）。
 *  DD-27：后端 gateway-api/.../packet_replay/sessions.rs 只内置 s1/s2；
 *  曾多列 s3，点开必然 404。以后端口径为准，后端新增时同步这里。 */
const BUILTIN_IDS = new Set(['s1', 's2'])

export function isBuiltin(id: string): boolean {
  return BUILTIN_IDS.has(id)
}

const STATUS_LABELS: Record<string, string> = {
  match: '一致', mismatch: '不一致', timeout: '超时', error: '失败',
}

export function statusLabel(s: string): string {
  return STATUS_LABELS[s] ?? s
}

/**
 * 回放帧状态 → 徽章颜色。
 * 通用表 timeout=danger，与本页「超时=中性灰」不同，故 match/mismatch/timeout 本地登记；
 * 其余（error 等）委托全站唯一映射表 utils/statusType，兜底保持 danger。
 */
const REPLAY_TAG_OVERRIDE: Record<string, TagType> = {
  match: 'success',
  mismatch: 'warning',
  timeout: 'info',
}

export function replayStatusTagType(s: string): TagType {
  const key = String(s ?? '').trim().toLowerCase()
  const hit = REPLAY_TAG_OVERRIDE[key]
  if (hit) return hit
  const t = baseStatusTagType(s)
  return t === 'info' ? 'danger' : t
}

export function usePacketReplay() {
  const { message, showToast } = useErrorDisplay()
  // 共享调试目标（审查#5）：回放目标总线与其它调试页保持一致
  const { busOptions, loading: targetsLoading, selectedBus: bus } = useDebugTargets()

  const sessions = ref<ReplaySession[]>([])
  const currentId = ref('')
  const session = ref<ReplaySession | null>(null)
  /** 加载/回放失败原因（带处置建议）：常驻页面，避免左侧列表空白却没有任何解释 */
  const replayError = ref('')
  const playing = ref(false)

  const timeoutMs = ref(1500)
  const intervalMs = ref(200)
  const stopOnMismatch = ref(false)
  /** P0-1：真实回放结果（契约 §4）。null 表示尚未执行 */
  const execResult = ref<ReplayExecuteResult | null>(null)
  /** P0-1b：流式逐帧中间态（summary 到达前用于渲染进度与逐帧状态） */
  const liveFrames = ref<ReplayFrameResult[]>([])
  /** 流式 started 事件给出的计划总帧数（进度条分母）；0 = 未开始/阻塞模式 */
  const streamTotal = ref(0)
  /**
   * DD-03：回放终态。只有 `completed` 才算跑完；aborted / failed / stopped
   * 必须在状态条显式提示，不能因为「收到了几帧 match」就显示成回放完成。
   * DD-49：终态值统一走 `toTaskTerminal`（与扫描/探测同一口径，未识别即 failed）。
   */
  const terminalState = ref<TaskTerminal | ''>('')
  /** 终态附加说明（失败原因），展示在状态条 */
  const terminalError = ref('')
  /** 用户是否主动点了停止（用于区分 stopped 与服务端 abort） */
  let userStopped = false
  // 会话详情请求序号守卫：快速切换会话时，旧会话的详情不得覆盖新会话
  const selectGuard = useRequestGuard()
  let abortCtrl: AbortController | null = null

  /** 进度条百分比（仅流式模式有计划总帧数） */
  const progressPct = computed(() =>
    streamTotal.value > 0 ? Math.min(100, Math.round((liveFrames.value.length / streamTotal.value) * 100)) : 0,
  )

  /**
   * 逐帧结果索引：优先全量汇总（最终态），否则用流式中间态（执行中）。
   * 合并顺序（先 liveFrames 后 execResult.results）与旧的 resultOf 查找语义一致。
   */
  const framesById = computed(() => {
    const map = new Map<string, ReplayFrameResult>()
    for (const r of liveFrames.value) map.set(r.packetId, r)
    for (const r of execResult.value?.results ?? []) map.set(r.packetId, r)
    return map
  })

  /**
   * 切换当前回放会话。
   *
   * 竞态守卫：快速切换两个会话时，先发的 getSession 可能后到 →
   * session.value 会显示上一个会话的详情，而 currentId 已是新会话，
   * 页面标题与详情和列表选中项对不上。
   */
  async function select(id: string) {
    const seq = selectGuard.next()
    currentId.value = id
    replayError.value = ''
    execResult.value = null
    liveFrames.value = []
    streamTotal.value = 0
    terminalState.value = ''
    terminalError.value = ''
    try {
      const s = (await getSession(id)) || null
      if (!selectGuard.isCurrent(seq)) return
      session.value = s
    } catch (e) {
      if (!selectGuard.isCurrent(seq)) return
      session.value = null
      replayError.value = message(e, '加载回放会话失败')
    }
  }

  async function doDelete(id: string) {
    const name = sessions.value.find((s) => s.id === id)?.name ?? id
    if (!(await confirmDelete(name))) return
    try {
      await deleteSession(id)
      sessions.value = sessions.value.filter((s) => s.id !== id)
      if (currentId.value === id) { currentId.value = ''; session.value = null }
      message('会话已删除', 'success')
    } catch (e) {
      replayError.value = message(e, '删除会话失败')
      showToast(e, '删除会话失败')
    }
  }

  async function saveAsNew() {
    if (!session.value) return
    let name: string
    try {
      ({ value: name } = await ElMessageBox.prompt('新会话名称：', '另存回放会话', {
        inputValue: session.value.name + '（副本）',
      }))
    } catch {
      return // 用户取消
    }
    if (!name) return
    const copy: ReplaySession = {
      ...session.value,
      id: 'u_' + Date.now().toString(36),
      name,
      createdAt: Date.now(),
    }
    try {
      const saved = await saveSession(copy)
      sessions.value.push(saved)
      currentId.value = saved.id
      session.value = saved
      message('会话已保存', 'success')
    } catch (e) {
      replayError.value = message(e, '复制会话失败')
      showToast(e, '复制会话失败')
    }
  }

  /**
   * 真实回放：默认走 P0-1b 流式（SSE 逐帧进度 + 可中途停止）；
   * 旧浏览器 / Tauri 桌面端回落 P0-1 阻塞模式（契约 p0-1b §6）。
   * 租约失败（总线被采集占用/离线）的 message 落入常驻 alert；
   * 帧级结果不打断整场回放，逐帧展示在列表上。
   */
  async function replay() {
    if (!session.value || !bus.value || playing.value) return
    const id = session.value.id
    const targetBus = bus.value
    playing.value = true
    replayError.value = ''
    execResult.value = null
    liveFrames.value = []
    streamTotal.value = 0
    terminalState.value = ''
    terminalError.value = ''
    userStopped = false
    const opts = { timeoutMs: timeoutMs.value, intervalMs: intervalMs.value, stopOnMismatch: stopOnMismatch.value }
    // 流式与阻塞是两条不同的写链路，失败文案按实际走的分支区分
    const useStream = canStreamReplay()
    try {
      if (useStream) {
        const ctrl = new AbortController()
        abortCtrl = ctrl
        await executeReplayStream(id, targetBus, opts, {
          signal: ctrl.signal,
          onStarted: (info) => {
            streamTotal.value = info.total
          },
          onFrame: (f) => { liveFrames.value.push(f) },
          onSummary: (r) => {
            execResult.value = r
            // 以服务端终态为准（DD-49：统一映射，未知终态按 failed，不报成完成）
            terminalState.value = toTaskTerminal(r.terminal)
          },
          onAborted: (info) => {
            terminalState.value = 'aborted'
            terminalError.value = `已执行 ${info.completed} / ${info.total} 帧（${info.reason === 'duration_cap' ? '服务端时长上限' : info.reason}）`
          },
          onFailed: (info) => {
            terminalState.value = 'failed'
            terminalError.value = info.message
            replayError.value = info.message
          },
        })
        // DD-03：流结束但没收到 summary，且也非上述终态（用户停止/连接异常）——
        // 不再本地重建"完成"汇总，只如实标注终态，逐帧结果仍在列表里。
        if (!terminalState.value) {
          terminalState.value = userStopped ? 'stopped' : 'failed'
          if (!userStopped) terminalError.value = '回放流意外结束，未收到服务端汇总'
        }
      } else {
        const r = await executeReplayBlocking(id, targetBus, opts)
        execResult.value = r
        const terminal = toTaskTerminal(r.terminal)
        terminalState.value = terminal
        if (terminal === 'aborted') {
          replayError.value = `回放已中止：已执行 ${r.total} / ${r.planned} 帧（服务端时长上限），结果仅含已执行部分`
        } else if (terminal === 'failed') {
          replayError.value = `回放中断：总线中途不可用，已执行 ${r.total} / ${r.planned} 帧`
        }
      }
    } catch (e) {
      const fallback = useStream ? '启动回放失败' : '执行回放失败'
      replayError.value = message(e, fallback)
      showToast(e, fallback)
      terminalState.value = 'failed'
      terminalError.value = replayError.value
    } finally {
      playing.value = false
      abortCtrl = null
    }
  }

  /** 停止回放：断流后后端在下一帧开始前停止，正在等待的帧按超时收尾（契约 §4） */
  function stopReplay() {
    userStopped = true
    abortCtrl?.abort()
  }

  function reset() {
    execResult.value = null
    liveFrames.value = []
    streamTotal.value = 0
    terminalState.value = ''
    terminalError.value = ''
    replayError.value = ''
  }

  // 离开页面时断流：后端据此停止回放并释放总线租约（契约 §4）
  onBeforeUnmount(() => abortCtrl?.abort())

  onMounted(async () => {
    try {
      sessions.value = await listSessions()
    } catch (e) {
      sessions.value = []
      replayError.value = message(e, '加载回放文件列表失败')
      return
    }
    if (sessions.value.length > 0) select(sessions.value[0]!.id)
  })

  return {
    busOptions,
    targetsLoading,
    bus,
    sessions,
    currentId,
    session,
    replayError,
    playing,
    timeoutMs,
    intervalMs,
    stopOnMismatch,
    execResult,
    liveFrames,
    streamTotal,
    progressPct,
    terminalState,
    terminalError,
    framesById,
    select,
    doDelete,
    saveAsNew,
    replay,
    stopReplay,
    reset,
  }
}
