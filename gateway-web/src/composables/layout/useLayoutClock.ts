import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useThemeStore } from '@/stores/theme'
import { useGatewayStore } from '@/stores/gateway'
import { useClock } from '@/composables/system/useClock'
import { usePolling } from '@/composables/core/usePolling'
import { healthComponentLabel, healthSeverity, healthStatusLabel, healthSnapshotDegraded } from '@/utils/healthStatus'
import { globalGet, scopedGet, scopedKey } from '@/utils/gatewayStorage'

/**
 * Layout 顶栏时钟 + 网关健康徽章 composable
 *
 * 从 Layout.vue 拆出，集中维护：
 *   - 顶栏时钟（每秒更新 HH:mm:ss）
 *   - 网关健康轮询（5s 周期，null=检测中 / true=正常 / false=异常）
 *   - 顶栏显示用户名 + 跨标签 localStorage 同步
 *   - 主题 store 桥接（isDark + toggleTheme）
 *
 * 设计要点：
 *   - 健康轮询严格遵守 "卸载后不再 setState"（闭包 flag 而非 watchHasUnmounted 钩子，
 *     避免与 setup 多次调用污染）
 *   - 内部 onMounted/onUnmounted 自管定时器与事件订阅，调用方无需关心清理
 */
export function useLayoutClock() {
  const themeStore = useThemeStore()
  const gateway = useGatewayStore()

  // ===== 时钟 =====
  // 1s 实时时钟委托 useClock（与 useSystem 共用实现，审计 P2-7 去重）：
  // now=顶栏时间字符串、nowTs=供 staleSeconds 等"多少秒前"计算的数值时钟
  const { now, nowTs, __clockTimerActive: clockActive } = useClock()

  // ===== 网关健康 =====
  /** 健康检查结果：null=尚未检测 / true=全部就绪 / false=降级 */
  const healthOk = ref<boolean | null>(null)
  /**
   * 上一次轮询是否真的连上了网关。
   *
   * 必须与 healthOk 分开：`gateway.loadHealth()` 在请求失败时返回**上一次成功的快照**
   * （store 只在成功分支写 health.value），所以仅凭快照 status 判断会把「网线拔了」
   * 误判成「网关正常」。失败信号只体现在 store.errors.health 上。
   * null=尚未检测。
   */
  const reachable = ref<boolean | null>(null)
  /**
   * 连续失败次数。单次失败可能是交换机瞬断/浏览器节流，立刻弹横幅会闪；
   * 横幅约定连续 2 次（约 10s）才提示。
   */
  const failStreak = ref(0)
  /** 最近一次轮询拿到的快照（可能来自失败时返回的旧值，仅用于展示组件明细） */
  const lastSnapshot = ref<{ status: string; components: Record<string, string>; queue_len?: number } | null>(null)
  /** 队列积压条数（快照自带）；无此字段的老后端返回 null，横幅降级为无数值文案 */
  const queueLen = computed(() => lastSnapshot.value?.queue_len ?? null)
  let unmounted = false

  async function checkHealth() {
    const resp = await gateway.loadHealth()
    if (unmounted) return
    // 请求失败时 store 会写入 errors.health；这是「连不上」的唯一可靠信号
    reachable.value = !gateway.errors.health
    failStreak.value = reachable.value ? 0 : failStreak.value + 1
    lastSnapshot.value = resp
    // 拿不到快照（网络错）时不能算健康：没有数据不等于一切正常
    healthOk.value = reachable.value && !!resp && !healthSnapshotDegraded(resp.components, resp.status)
  }

  /**
   * 健康轮询统一走 usePolling（P2 收敛）：标签页隐藏时暂停，卸载自动清理。
   * loadHealth 失败只写 errors.health 不抛错，这里显式抛出让 usePolling 感知失败——
   * 网关失联后轮询按指数退避降频（5s → 最长 40s），不再白打注定失败的请求。
   */
  async function pollHealth() {
    await checkHealth()
    if (gateway.errors.health) throw new Error('gateway health check failed')
  }
  const { running: healthPolling } = usePolling(pollHealth, 5000)

  const healthOkClass = computed(() => {
    if (healthOk.value === null) return { healthy: false, warning: false, unknown: true }
    if (!healthOk.value) return { healthy: false, warning: false, unknown: false }
    // 可达但有异常项：黄色警示档，不再显示绿色「服务在线」
    return { healthy: anomalyCount.value === 0, warning: anomalyCount.value > 0, unknown: false }
  })
  /**
   * 顶栏徽章文案：正常态明确写「服务在线」（原「网关正常」容易被读成设备/链路都正常）；
   * 有异常时优先显示条数，点开下拉看明细。
   */
  const healthOkText = computed(() => {
    if (reachable.value === false) return '失联'
    if (healthOk.value === null) return '检测中...'
    if (anomalyCount.value > 0) return `${anomalyCount.value} 项异常`
    // 可达但整体 status!=ok（例如某组件 down 却没被归入明细）：兜底文案，不能显示「在线」
    if (!healthOk.value) return '服务异常'
    return '服务在线'
  })

  /**
   * 异常条数：已按组件严重度判定的降级组件 + 队列积压。
   *
   * 数据源都来自同一次 /health/ready 快照，随 5s 轮询自动联动；
   * 队列积压（queue_len > 0）若已体现为 queue 组件降级，不重复计数——
   * 否则「1 项异常」会让用户以为有两个独立故障。
   */
  const anomalyCount = computed(() => {
    const base = degradedComponents.value.length
    const q = queueLen.value ?? 0
    const comps = lastSnapshot.value?.components
    const queueDegraded = !!comps && 'queue' in comps && healthSeverity(comps.queue) !== 'ok'
    return base + (q > 0 && !queueDegraded ? 1 : 0)
  })

  /** 网关不可达（断网/进程退出/地址错误）：页面上的数据可能都是历史值 */
  const gatewayUnreachable = computed(() => reachable.value === false)

  /**
   * 降级的组件明细，形如 ['MQTT 客户端：不可用']。
   *
   * 组件名与状态名统一走 utils/healthStatus（后端 /health/ready 的真实取值），
   * 不再把英文原词直接甩给现场工程师。
   */
  const degradedComponents = computed(() =>
    Object.entries(lastSnapshot.value?.components ?? {})
      .filter(([, st]) => healthSeverity(st) !== 'ok')
      .map(([name, st]) => `${healthComponentLabel(name)}：${healthStatusLabel(st)}`),
  )

  /**
   * 网关可达但组件降级。
   *
   * 按组件严重度判定，而不是直接用后端聚合的 status：后端对任何非 ok 组件都判
   * degraded，直接沿用会多出一条与组件明细口径不符的「组件异常」横幅。
   */
  const gatewayDegraded = computed(
    () =>
      reachable.value === true &&
      !!lastSnapshot.value &&
      healthSnapshotDegraded(lastSnapshot.value.components, lastSnapshot.value.status),
  )

  /**
   * 距上次成功获取健康快照的秒数；从未成功过返回 null。
   * 依赖 nowTs 每秒刷新，保证横幅上的「N 秒前」自己会走。
   */
  const staleSeconds = computed(() => {
    const ts = gateway.lastUpdated.health ?? 0
    if (!ts) return null
    return Math.max(0, Math.round((nowTs.value - ts) / 1000))
  })

  // ===== 顶栏用户名 =====
  const username = ref('')
  /** 用户名按网关注入：切网关后不应仍显示上一台网关的登录名 */
  const USERNAME_KEY = 'gw_username'
  function readUsername() {
    username.value = scopedGet(USERNAME_KEY) || ''
  }

  // ===== 主题 =====
  const isDark = computed(() => themeStore.isDark)
  function toggleTheme() {
    themeStore.toggleTheme()
  }

  // ===== 跨标签 storage 同步 =====
  function onStorage(e: StorageEvent) {
    // 用户名是隔离 key（带网关前缀），主题是全局 key，两者比对口径不同
    if (e.key === scopedKey(USERNAME_KEY) || e.key === null) readUsername()
    if (e.key === 'gw_theme' || e.key === null) {
      const next = globalGet('gw_theme') === 'dark'
      if (next !== themeStore.isDark) themeStore.setDark(next)
    }
  }

  // ===== 生命周期（自管） =====
  // 健康轮询由上方 usePolling 自管（autoStart：挂载即首次检测），这里只留用户名/主题同步
  onMounted(() => {
    readUsername()
    window.addEventListener('storage', onStorage)
  })

  onUnmounted(() => {
    unmounted = true
    window.removeEventListener('storage', onStorage)
  })

  return {
    // 时钟
    now,
    nowTs,
    // 健康
    healthOk,
    healthOkClass,
    healthOkText,
    anomalyCount,
    checkHealth,
    // 连通性（全局横幅用）
    gatewayUnreachable,
    gatewayDegraded,
    degradedComponents,
    staleSeconds,
    failStreak,
    queueLen,
    // 用户名
    username,
    readUsername,
    // 主题
    isDark,
    toggleTheme,
    // 测试钩子：暴露 unmounted flag 与定时器引用，便于单元测试覆盖清理逻辑
    // （时钟定时器已移交 useClock 自管，由其 __clockTimerActive 断言）
    __internal: {
      get unmounted() { return unmounted },
      get clockTimerActive() { return clockActive() },
      get healthTimerActive() { return healthPolling.value },
    },
  }
}
