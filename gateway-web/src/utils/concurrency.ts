/**
 * 前端并发池（唯一实现）
 *
 * # 为什么收成一个模块
 *
 * 批量操作在 11 个文件里各写一遍「N 个 worker 抢一条共享队列」：
 *
 *   ```ts
 *   const queue = [...items]
 *   async function worker() {
 *     while (!stopped()) {
 *       const it = queue.shift()
 *       if (!it) break
 *       try { await handle(it) } catch { record(it) }
 *     }
 *   }
 *   await Promise.all(Array.from({ length: Math.min(CONCURRENCY, items.length) }, () => worker()))
 *   ```
 *
 * 这段样板重复了 11 次，且**每一份的手势都不同**：上限 4/5/6/8/16 各不相同，
 * 停止条件分别是 `isUnmounted()`、`unmounted && seq === querySeq`、`cancelled`、
 * `signal.aborted` + 整体超时预算；聚合方式也分三类——计数（成功/失败几个）、
 * 明细数组（失败 id + 原因）、进度推进（每完成一条 completed++）。
 *
 * # 这里只统一「调度」，不统一「语义」
 *
 * 本模块负责且仅负责三件事：
 * 1. 把并发数钉在上限内（`concurrency`）；
 * 2. 在**领取下一条之前**检查停止条件（`shouldStop`）——
 *    已��在飞的任务不被打断，这与原实现逐字一致；
 * 3. 全部 worker 跑完后才 resolve。
 *
 * 停止条件的**求值时机**与**触发后的行为**由调用方决定：
 * - `useHistoryQuery` 在新一轮查询发起后停掉旧轮次（`seq === querySeq`）；
 * - `useGatewayDiscovery` 在 abort 或超出整体扫描预算后停（`signal.aborted` + 预算）；
 * - `useDiscoverWizard` 的导入没有停止条件，不传 `shouldStop` 即可。
 *
 * 单条失败如何计数、失败明细怎么收集、进度怎么推进，**仍由各调用方在自己的
 * 回调里完成**——因此本模块不吞异常：回调抛出的异常会在全部 worker 结束后
 * 以第一个错误重新抛出（`Promise.allSettled` + 重抛），避免遗留未处理的 rejection。
 *
 * # 结果顺序
 *
 * 回调里往共享数组 `push` 的顺序是**完成顺序**，不是入参顺序——与迁移前逐字一致。
 * 依赖顺序的地方（如 `useHistoryQuery` 用 `results[0]` 当默认选中点位）
 * 迁移后行为不变，不要在这里"顺手修正"。
 */

/** 并发上限默认值（各调用方基本都会显式传自己的上限） */
const DEFAULT_CONCURRENCY = 8

export interface ConcurrentOptions {
  /**
   * 同时在飞的任务数上限。
   * 实际并发数取 `min(concurrency, 任务总数)`；任务总数为 0 时不启动任何 worker。
   */
  concurrency?: number
  /**
   * 停止条件，**在领取下一条任务之前**求值。
   *
   * 返回 true 表示不再领取新任务；已在飞的任务照常跑完（不打断），
   * 整体 resolve 时间不变。这与迁移前各调用方的 `while (!stopped())` 语义一致。
   */
  shouldStop?: () => boolean
}

/**
 * 以受限并发跑一批任务。
 *
 * @param items  待处理项
 * @param task   单项处理函数；抛出的异常会让整池以该错误 reject
 * @param options 并发上限与停止条件
 */
export async function runConcurrent<T>(
  items: readonly T[],
  task: (item: T, index: number) => Promise<void>,
  options: ConcurrentOptions = {},
): Promise<void> {
  const total = items.length
  if (total === 0) return
  const limit = Math.max(1, Math.min(options.concurrency ?? DEFAULT_CONCURRENCY, total))
  const shouldStop = options.shouldStop
  // 共享游标：谁空出来谁领下一条，与迁移前的 queue.shift() 等价
  let cursor = 0

  async function worker(): Promise<void> {
    while (cursor < total) {
      if (shouldStop?.() === true) return
      const index = cursor++
      // index < total 由上面的循环条件保证，这里非空断言只是满足 noUncheckedIndexedAccess
      await task(items[index]!, index)
    }
  }

  const results = await Promise.allSettled(
    Array.from({ length: limit }, () => worker()),
  )
  // 用 allSettled 而非 all：all 会在首个 reject 时立刻抛出，
  // 其余 worker 的异常变成未处理 rejection（Node 会在进程退出时报错）。
  // 这里等所有 worker 收尾后再重抛第一个错误。
  const firstRejected = results.find((r) => r.status === 'rejected')
  if (firstRejected && firstRejected.status === 'rejected') {
    throw firstRejected.reason
  }
}
