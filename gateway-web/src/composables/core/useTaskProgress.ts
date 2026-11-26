/**
 * 流式任务收尾口径（DD-49）
 *
 * 背景：参数探测、寄存器扫描、报文回放三条流式链路各写一套「轮询 + 收尾」，
 * 终态判定分散——`done` / `completed` / `aborted` / `stopped` 各认各的，
 * 曾出现过「失败被当成完成」（DD-03）与「永远转圈」（DD-02）。
 *
 * 本模块只做两件事：
 * 1. 一份终态映射：只有显式 `running` 才算进行中，未识别的状态一律按 `failed`，
 *    宁可多报一次失败，也不把失败报成完成；
 * 2. 一个轮询收尾器 `usePolledTask`：统一处理「轮询到非 running 即收尾、连续失败
 *    上限、收尾只回调一次、主动 end 后迟到响应不得再收尾」。
 *
 * 报文回放走 SSE 而不是轮询，只消费这里的 `toTaskTerminal`，保证三页终态同源。
 */
import { ref, type Ref } from 'vue'
import { usePolling } from './usePolling'

/** 统一终态：`running` 不是终态，故不在此列 */
export type TaskTerminal = 'completed' | 'aborted' | 'failed' | 'stopped'

const TERMINAL_MAP: Record<string, TaskTerminal> = {
  // 扫描 / 探测的 done 与回放的 completed 是同一件事
  done: 'completed',
  completed: 'completed',
  success: 'completed',
  ok: 'completed',
  aborted: 'aborted',
  cancelled: 'aborted',
  canceled: 'aborted',
  stopped: 'stopped',
  failed: 'failed',
  error: 'failed',
}

function norm(raw: unknown): string {
  return String(raw ?? '').trim().toLowerCase()
}

/** 是否仍在跑：只有显式 running 才是进行中，其余一律进入终态（失败不得报成完成） */
export function isTaskRunning(raw: unknown): boolean {
  return norm(raw) === 'running'
}

/** 后端状态 / SSE 终态 → 全站统一终态；无法识别的值按 failed 处理 */
export function toTaskTerminal(raw: unknown): TaskTerminal {
  return TERMINAL_MAP[norm(raw)] ?? 'failed'
}

export interface PolledTaskOptions<T> {
  /** 轮询间隔（ms） */
  interval: number
  /** 拉取任务结果；返回空表示本轮无结果 */
  fetch: () => Promise<T | null | undefined>
  /** 从结果取状态字符串（running / done / failed / aborted…） */
  statusOf: (r: T) => unknown
  /** 每轮成功都回调（含进行中），用于刷新进度 */
  onUpdate?: (r: T) => void
  /** 进入终态时回调一次（completed / aborted / failed / stopped） */
  onSettled: (r: T, terminal: TaskTerminal) => void
  /** 连续拉取失败达到上限时回调（此后不再轮询） */
  onFetchFailures: (e: unknown) => void
  /** 连续失败上限，默认 3 次 */
  maxFailures?: number
  /** fetch 返回空是否算一次失败（默认 false：丢弃该轮继续轮询） */
  requireResult?: boolean
}

export interface PolledTaskReturn {
  /** 是否处于「已开始且未收尾」状态（页面按钮据此置灰） */
  polling: Readonly<Ref<boolean>>
  /** 开始轮询；runNow=true 时立即取一次（默认等一个周期） */
  begin: (runNow?: boolean) => void
  /** 主动收尾（用户删除/切换任务）：停止轮询，迟到响应不得再触发 onSettled */
  end: () => void
}

export function usePolledTask<T>(opts: PolledTaskOptions<T>): PolledTaskReturn {
  const polling = ref(false)
  const maxFailures = opts.maxFailures ?? 3
  let failures = 0
  /** 收尾闩：无论来自终态、失败上限还是主动 end，都只允许收尾一次 */
  let finished = true

  function settle(r: T, terminal: TaskTerminal) {
    if (finished) return
    finished = true
    polling.value = false
    stop()
    opts.onSettled(r, terminal)
  }

  async function tick() {
    if (finished) return
    try {
      const r = await opts.fetch()
      // 请求在飞期间可能已被 end()/上一轮收尾，迟到的成功不得再改状态
      if (finished) return
      if (r == null) {
        if (opts.requireResult) throw new Error('任务结果不存在')
        return
      }
      failures = 0
      opts.onUpdate?.(r)
      const status = opts.statusOf(r)
      if (!isTaskRunning(status)) settle(r, toTaskTerminal(status))
    } catch (e) {
      if (finished) return
      if (++failures >= maxFailures) {
        finished = true
        polling.value = false
        stop()
        opts.onFetchFailures(e)
      }
    }
  }

  // 失败上限由业务自管（连续 N 次即停），关闭内置退避；标签页隐藏暂停仍生效
  const { start, stop } = usePolling(tick, opts.interval, { autoStart: false, backoff: false })

  function begin(runNow = false) {
    failures = 0
    finished = false
    polling.value = true
    // 幂等：重复开始时先停掉上一轮订阅，避免叠加定时器
    stop()
    start(runNow)
  }

  function end() {
    finished = true
    polling.value = false
    stop()
  }

  return { polling: polling as Readonly<Ref<boolean>>, begin, end }
}
