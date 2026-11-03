/**
 * 报文回放页状态机（从 PacketReplay.vue 拆出）
 *
 * 负责：回放文件列表/会话加载、预置会话保护、真实回放（默认 P0-1b 流式，
 * 旧浏览器 / Tauri 回落 P0-1 阻塞）、逐帧中间态与汇总、停止/重置/另存。
 * 页面组件只保留布局与事件接线，逐帧列表与运行状态条在 packet/parts/ 下渲染。
 *
 * 为什么状态集中在这里：回放是「一条写链路 + 一份执行结果」——列表选中、
 * 进度条、逐帧状态、停止按钮都读同一份状态，父子各维护一份必然串台。
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
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
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import { statusTagType as baseStatusTagType, type TagType } from '@/utils/statusType'

/** 预置会话不可删（后端也拒绝，前端灰色 + tooltip） */
const BUILTIN_IDS = new Set(['s1', 's2', 's3'])

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

export function statusTagType(s: string): TagType {
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
  /** 流式 started 事件的开始时间戳（中断收尾时用于本地汇总） */
  const streamStartedAt = ref(0)
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

  async function select(id: string) {
    currentId.value = id
    replayError.value = ''
    execResult.value = null
    liveFrames.value = []
    streamTotal.value = 0
    try {
      session.value = (await getSession(id)) || null
    } catch (e) {
      session.value = null
      replayError.value = message(e, '加载回放会话失败')
    }
  }

  async function doDelete(id: string) {
    if (!confirm(`确定删除「${sessions.value.find((s) => s.id === id)?.name ?? id}」？`)) return
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
    const name = prompt('新会话名称：', session.value.name + '（副本）')
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
    streamStartedAt.value = 0
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
            streamStartedAt.value = info.startedAtMs
          },
          onFrame: (f) => { liveFrames.value.push(f) },
          onSummary: (r) => { execResult.value = r },
          onFailed: (info) => { replayError.value = info.message },
          // duration_cap aborted：落到下面用已收帧重建本地汇总
        })
        // 流收尾但无 summary（用户停止 / 硬上限 / failed）：用已收帧重建汇总展示
        if (!execResult.value && liveFrames.value.length > 0) {
          execResult.value = summarizeFrames(id, targetBus, liveFrames.value)
        }
      } else {
        execResult.value = await executeReplayBlocking(id, targetBus, opts)
      }
    } catch (e) {
      const fallback = useStream ? '启动回放失败' : '执行回放失败'
      replayError.value = message(e, fallback)
      showToast(e, fallback)
    } finally {
      playing.value = false
      abortCtrl = null
    }
  }

  /** 停止回放：断流后后端在下一帧开始前停止，正在等待的帧按超时收尾（契约 §4） */
  function stopReplay() {
    abortCtrl?.abort()
  }

  /** 用已到达的逐帧结果重建本地汇总（流式中断/硬上限/failed 收尾时） */
  function summarizeFrames(id: string, busName: string, frames: ReplayFrameResult[]): ReplayExecuteResult {
    return {
      sessionId: id,
      bus: busName,
      startedAtMs: streamStartedAt.value || Date.now(),
      finishedAtMs: Date.now(),
      total: frames.length,
      match: frames.filter((f) => f.status === 'match').length,
      mismatch: frames.filter((f) => f.status === 'mismatch').length,
      failed: frames.filter((f) => f.status !== 'match' && f.status !== 'mismatch').length,
      results: [...frames],
    }
  }

  function reset() {
    execResult.value = null
    liveFrames.value = []
    streamTotal.value = 0
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
    framesById,
    select,
    doDelete,
    saveAsNew,
    replay,
    stopReplay,
    reset,
  }
}
