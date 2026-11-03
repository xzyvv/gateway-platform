import { ref, onUnmounted } from 'vue'
import { ElLoading, ElMessage, ElMessageBox } from 'element-plus'
import {
  isTauri, loadGatewayConfig, setActiveGateway, saveGatewayConfig,
  scanGatewayLan, checkUrlHealth,
  getInflightCount, onInflightCountChange, abortAllInflight,
  type GatewayEntry, type DiscoveredGateway,
} from '@/api'
import { extractErrorMessage } from '@/utils/errorHandler'

/**
 * 网关切换 + LAN 扫描 composable
 *
 * 从 Layout.vue 拆出，集中维护：
 *   - 桌面端（Tauri）多网关列表 / 当前激活网关
 *   - 网关切换流程（drain in-flight → 持久化 → liveness 探测 → reload）
 *   - LAN 网关扫描 + 弹窗展示 + 一键添加
 *   - 全局 in-flight 计数订阅（用于顶栏 banner）
 *
 * 设计要点：
 *   - 内部自动注册 onUnmounted 清理，无需外部手动管理定时器/订阅
 *   - isTauriEnv 在创建时确定（isTauri() 同步返回），不响应运行时切换
 *   - performGatewaySwitch 内嵌 ElLoading.service，不需要外部包裹
 *
 * 使用方（Layout.vue）只需：
 *   const sw = useGatewaySwitcher()
 *   // 模板里：sw.gateways / sw.activeGatewayName / sw.isSwitchingGateway ...
 *   // 事件里：sw.onGatewayCmd(cmd) / sw.performLanScan() / sw.addDiscoveredGateway(gw)
 */
export function useGatewaySwitcher() {
  const isTauriEnv = isTauri()

  const gateways = ref<GatewayEntry[]>([])
  const activeGatewayName = ref('')
  const activeGatewayUrl = ref('')
  const isSwitchingGateway = ref(false)

  // in-flight 请求计数 banner
  const inflightBanner = ref(false)
  let unsubscribeInflight: (() => void) | null = null

  // LAN 扫描状态
  const isScanning = ref(false)
  const scanResults = ref<DiscoveredGateway[]>([])
  const showScanDialog = ref(false)

  async function refreshGateways() {
    if (!isTauriEnv) return
    const cfg = await loadGatewayConfig()
    gateways.value = cfg.gateways || []
    activeGatewayName.value = cfg.active || ''
    const gw = gateways.value.find((g) => g.name === activeGatewayName.value)
    activeGatewayUrl.value = gw ? gw.url : ''
  }

  /**
   * A3 修复：等所有 in-flight 请求自然结束（最多 timeoutMs），超时则 abort。
   * 原实现切网关直接 reload()，导致用户提交的"保存配置"等写请求在 in-flight 时
   * 被 Tauri 端仍在后台执行，最终可能写入错误的网关。
   */
  async function waitForInflightDrain(timeoutMs = 3000): Promise<void> {
    const t0 = Date.now()
    while (Date.now() - t0 < timeoutMs) {
      if (getInflightCount() === 0) return
      await new Promise((r) => setTimeout(r, 50))
    }
    if (getInflightCount() > 0) {
      abortAllInflight('gateway switch timeout')
      await new Promise((r) => setTimeout(r, 200))
    }
  }

  /** 探测新网关的 liveness（仅 desktop 模式有意义）。 */
  async function probeNewGateway(url: string): Promise<boolean> {
    // Tauri 模式：通过 Rust 健康检测命令，避免 webview 直连 HTTP（CSP 收紧后必需）
    if (isTauri()) {
      const health = await checkUrlHealth(url)
      return health?.reachable ?? false
    }
    try {
      const controller = new AbortController()
      const tid = setTimeout(() => controller.abort(), 4000)
      const resp = await fetch(`${url.replace(/\/$/, '')}/health/live`, {
        signal: controller.signal,
      })
      clearTimeout(tid)
      return resp.ok
    } catch {
      return false
    }
  }

  /**
   * 统一切网关入口。
   *   1. 锁定下拉防并发
   *   2. 等 in-flight 收尾
   *   3. desktop 持久化新激活项
   *   4. 探测新网关 liveness
   *   5. 通过后才 reload
   */
  async function performGatewaySwitch(targetName: string, targetUrl?: string): Promise<void> {
    if (isSwitchingGateway.value) return
    isSwitchingGateway.value = true

    const loading = ElLoading.service({
      lock: true,
      text: `正在切换到网关 "${targetName}"...`,
      background: 'rgba(0, 0, 0, 0.4)',
    })

    try {
      await waitForInflightDrain(3000)
      await setActiveGateway(targetName)
      await refreshGateways()

      const url = targetUrl || gateways.value.find((g) => g.name === targetName)?.url
      if (!url) throw new Error(`未找到网关 ${targetName} 的 URL`)
      const reachable = await probeNewGateway(url)
      if (!reachable) throw new Error(`新网关 ${targetName} 不可达（health/live 未响应）`)

      loading.close()
      ElMessage.success(`已切换到网关 "${targetName}"`)
      setTimeout(() => window.location.reload(), 200)
    } catch (e) {
      loading.close()
      const msg = extractErrorMessage(e)
      ElMessage.error(`切换失败：${msg}`)
    } finally {
      isSwitchingGateway.value = false
    }
  }

  async function onGatewayCmd(cmd: string) {
    if (cmd === 'add') {
      try {
        const { value } = await ElMessageBox.prompt(
          '请输入网关地址，如 http://192.168.1.100:8080',
          '新增网关',
          {
            confirmButtonText: '确定',
            cancelButtonText: '取消',
            inputPattern: /^https?:\/\/.+/,
            inputErrorMessage: '请输入有效的 http(s) 地址',
            inputMaxLength: 256,
          } as Record<string, unknown>,
        )
        const url = value.trim()
        const name = url
        // 去重：同名则覆盖
        const list = gateways.value.filter((g) => g.name !== name)
        list.push({ name, url })
        await saveGatewayConfig({ gateways: list, active: name })
        await refreshGateways()
        await performGatewaySwitch(name, url)
      } catch { /* 用户取消 */ }
    } else if (cmd === 'scan') {
      await performLanScan()
    } else if (cmd.startsWith('switch:')) {
      const name = cmd.slice('switch:'.length)
      await performGatewaySwitch(name)
    }
  }

  /** 扫描局域网，结果展示在弹窗里 */
  async function performLanScan() {
    if (!isTauriEnv || isScanning.value) return
    isScanning.value = true
    showScanDialog.value = true
    scanResults.value = []
    try {
      const results = await scanGatewayLan(8080)
      scanResults.value = results
      if (results.length > 0) {
        ElMessage.success(`发现 ${results.length} 个网关`)
      } else {
        ElMessage.warning('未发现网关设备')
      }
    } catch (e) {
      const msg = extractErrorMessage(e)
      ElMessage.error(`扫描失败：${msg}`)
    } finally {
      isScanning.value = false
    }
  }

  /** 主动关闭扫描弹窗（弹窗 footer 关闭按钮用） */
  function closeScanDialog() {
    showScanDialog.value = false
  }

  /** 把扫描到的网关加入配置列表（去重） */
  async function addDiscoveredGateway(gw: DiscoveredGateway) {
    const url = `http://${gw.ip}:${gw.port}`
    if (gateways.value.some((g) => g.url === url)) {
      ElMessage.info('该网关已在列表中')
      return
    }
    const name = gw.name || gw.ip
    const list = [...gateways.value, { name, url }]
    await saveGatewayConfig({ gateways: list, active: activeGatewayName.value || name })
    await refreshGateways()
    ElMessage.success(`已新增网关 "${name}"`)
  }

  // 桌面端订阅 in-flight 计数（Web 模式不订阅，避免泄漏）
  if (isTauriEnv) {
    unsubscribeInflight = onInflightCountChange((n) => {
      inflightBanner.value = n > 0
    })
  }

  onUnmounted(() => {
    if (unsubscribeInflight) {
      unsubscribeInflight()
      unsubscribeInflight = null
    }
  })

  return {
    // 环境
    isTauriEnv,
    // 网关列表
    gateways,
    activeGatewayName,
    activeGatewayUrl,
    isSwitchingGateway,
    refreshGateways,
    performGatewaySwitch,
    onGatewayCmd,
    // liveness 探测（外部一般不需要，暴露便于测试覆盖 web 分支的 fetch + signal 路径）
    probeNewGateway,
    // LAN 扫描
    isScanning,
    scanResults,
    showScanDialog,
    performLanScan,
    addDiscoveredGateway,
    closeScanDialog,
    // 全局计数
    inflightBanner,
  }
}