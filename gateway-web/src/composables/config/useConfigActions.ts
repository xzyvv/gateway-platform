import { ref, reactive, computed, watch, onMounted, onUnmounted } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { confirmAction } from '@/utils/confirm'
import { extractErrorMessage, failureMessage } from '@/utils/errorHandler'
import { registerDirtyGuard } from '@/utils/dirtyGuard'
import {
  postConfig,
  exportConfig as apiExport,
  reloadConfig,
} from '@/api/config'
import { setToken } from '@/api'
import type { GatewayConfig } from '@/api/types'
import { validateConfig, type ConfigIssue } from '@/utils/configValidation'
import {
  createSeedConfig,
  defaultWeb,
  defaultMqtt,
  unknownTopKeys,
  type ConfigForm,
} from '@/utils/configDefaults'
import { useConfigJson } from './useConfigJson'
import { scopedGet, scopedSet, scopedRemove } from '@/utils/gatewayStorage'
import { useWebhook } from '@/composables/uplink/useWebhook'
import { protocolDefaultTimeoutMs } from '@/composables/devices/useProtocol'
import { useRoute } from 'vue-router'
import { triggerDownload } from '@/utils/exportCsv'

/**
 * 运行参数页面状态与操作封装
 *
 * 把 Config.vue 中分散的状态（dialog 显示、JSON 同步）与按钮逻辑（保存/重载/导出/预览）
 * 集中到一个 composable，主组件只剩 template 与标签切换。
 *
 * 数据骨架/默认值在 [../utils/configDefaults]；「原始 JSON」文本态与同步在 [./useConfigJson]。
 *
 * @returns 见下方字段说明
 */
export function useConfigActions() {
  const route = useRoute()

  // ===== 当前 tab =====
  // 支持 ?section=mqtt 这类深链：其他页面的「去配置 MQTT」直接落到对应分节，
  // 而不是打开运行参数总页让用户再自己找（默认仍是 buses）。
  const CONFIG_TABS = ['buses', 'web', 'mqtt', 'ntp', 'telemetry', 'webhook', 'raw'] as const
  const sectionParam = route.query.section
  const activeTab = ref<string>('buses')
  if (typeof sectionParam === 'string' && (CONFIG_TABS as readonly string[]).includes(sectionParam)) {
    activeTab.value = sectionParam
  }

  // ===== dialog 显示状态 =====
  const showValidate = ref(false)
  const showDiff = ref(false)
  const reloading = ref(false)
  // 修复(审查#B7)：保存操作「在飞」标志。原实现无任何在飞保护，保存按钮未绑定
  // loading/disabled，双击会叠加两次 confirmAction + postConfig + 热重载，
  // 导致采集被中断两次、成功提示重复、审计重复记录。
  const saving = ref(false)
  // 修复(审查#14)：配置加载失败标记。加载失败时界面停留在默认/空配置，
  // 若不拦截保存，操作员按一下保存就会把网关上的真实配置整体覆盖成默认值。
  const loadFailed = ref(false)

  // ===== config 主数据 =====
  const config = reactive<ConfigForm>(createSeedConfig())

  // API 边界直接使用 config：表单模型与 GatewayConfig 同构，无需断言
  const gatewayConfig = computed<GatewayConfig>(() => config)

  // ===== Token 处理：缓存后端脱敏前的原始值，保存时恢复 =====
  const originalToken = ref('')

  // ===== 卸载标记（异步回调与提示的静默开关）=====
  let unmounted = false
  const isUnmounted = () => unmounted

  /** 检测未知顶层键，触发一次性 toast 警告（不阻断；与 ts-rs 字段漂移兼容） */
  function warnUnknownTopKeys(source: Record<string, unknown>, origin: string) {
    const unknown = unknownTopKeys(source)
    if (unknown.length === 0) return
    // 仅在组件挂载期间触发；onUnmounted 之后的回调不再打扰（防泄漏噪音）
    if (isUnmounted()) return
    ElMessage.warning(
      `${origin} 含有 GatewayConfig 未识别的键: ${unknown.join(', ')}；后端将忽略这些字段`,
    )
  }

  // 未保存更改守卫：加载/保存成功后基线同步为"干净"；此后任何配置字段变更即标脏，
  // 路由离开（router.beforeEach）会弹确认，防止改到一半的配置静默丢失
  // Bug-W6 (fix)：用 route.path 作为 scope，弹框时只清当前路由脏标记。
  const dirtyGuard = registerDirtyGuard(route.path)
  let dirtyBaseline = ''
  function markClean() {
    dirtyBaseline = JSON.stringify(config)
    dirtyGuard.reset()
  }
  watch(() => JSON.stringify(config), (s) => {
    if (s === dirtyBaseline) dirtyGuard.reset()
    else dirtyGuard.mark()
  })
  onUnmounted(() => dirtyGuard.unregister())

  // ===== 原始 JSON 标签（文本态与双向同步交由 useConfigJson）=====
  const {
    rawText, parseError, parseErrorMessage, rawJsonHtml,
    syncConfigToRaw, onRawJsonInput, syncScroll,
  } = useConfigJson({ config, activeTab, onUnknownTopKeys: warnUnknownTopKeys })

  const tokenStatusText = computed(() => {
    const v = config.web?.api_token
    if (!v) return '保留原值'
    if (v === originalToken.value) return '当前值'
    return '已修改'
  })

  onMounted(() => {
    unmounted = false
    loadConfig()
  })
  onUnmounted(() => {
    unmounted = true
  })

  function resetRawText() {
    syncConfigToRaw()
    ElMessage.success('已恢复为当前配置')
    markClean()
  }

  // ===== Bus CRUD =====
  function addBus() {
    // 默认端口按 /dev/ttyUSB 前缀递增选下一个未被占用的编号，避免连续点「新增」
    // 产生同名端口（端口名重复会被 validateConfig 阻断，体验不佳）。
    // 已用编号集合仅看 /dev/ttyUSB\d+；非该前缀的端口（如 /dev/ttyS1）不阻塞递增。
    const used = new Set<number>()
    for (const b of config.buses) {
      const m = /^\/dev\/ttyUSB(\d+)$/.exec(b.port_name || '')
      if (m) used.add(Number(m[1]))
    }
    let nextIdx = 0
    while (used.has(nextIdx)) nextIdx++
    const portName = `/dev/ttyUSB${nextIdx}`

    // 字段补全后端 serde/Default 默认值（channel_capacity 运行时 None 即按 128 处理）
    config.buses.push({
      port_name: portName,
      baud_rate: 9600,
      parity: 'none',
      data_bits: 8,
      stop_bits: 1,
      timeout_ms: protocolDefaultTimeoutMs('modbus'),
      protocol: 'modbus',
      retry_count: 2,
      retry_delay_ms: 200,
      de_pin: null,
      modbus_mode: 'rtu',
      dlt645_version: '2007',
      variant: null,
      asdu_addr_len: null,
      opcua: null,
      tcp_addr: null,
      tcp_concurrent: null,
      channel_capacity: 128,
      reconnect_backoff: { seconds: [1, 2, 5, 10, 30], jitter_ratio: 0 },
    })
  }

  function removeBus(index: number) {
    config.buses.splice(index, 1)
  }

  // ===== 本地草稿（P2b）=====
  // 现场配置一条总线/一批点位往往要十几分钟，中途去看手册、查端口、断电重启都会离开本页。
  // 路由守卫只能「拦截离开」，拦不住浏览器崩溃、断电、误关标签页。
  // 这里额外提供手动「存为草稿」到本地（按激活网关隔离），下次进入本页时提示恢复。
  // 草稿只存浏览器本地，不写入网关、不参与校验，恢复后仍需「保存并应用」才生效。
  const DRAFT_KEY = 'config_draft'
  const hasDraft = ref(false)
  const draftSavedAt = ref<number | null>(null)
  let draftConfig: GatewayConfig | null = null

  function readDraft(): { savedAt: number; config: GatewayConfig } | null {
    try {
      const raw = scopedGet(DRAFT_KEY)
      if (!raw) return null
      const parsed = JSON.parse(raw) as { savedAt?: number; config?: GatewayConfig }
      if (!parsed?.config) return null
      return { savedAt: Number(parsed.savedAt) || 0, config: parsed.config }
    } catch {
      return null
    }
  }

  function saveDraft() {
    try {
      scopedSet(DRAFT_KEY, JSON.stringify({ savedAt: Date.now(), config }))
      hasDraft.value = true
      draftSavedAt.value = Date.now()
      ElMessage.success('草稿已保存到本地（未写入网关，恢复后仍需「保存并应用」）')
    } catch {
      ElMessage.error('草稿保存失败：浏览器本地存储不可用')
    }
  }

  function restoreDraft() {
    if (!draftConfig) return
    Object.assign(config, JSON.parse(JSON.stringify(draftConfig)))
    syncConfigToRaw()
    // 恢复后保持「脏」状态：必须再点保存才会真正生效，避免用户误以为已提交
    ElMessage.success('已恢复本地草稿，确认无误后请点「保存并应用」')
  }

  function discardDraft() {
    scopedRemove(DRAFT_KEY)
    hasDraft.value = false
    draftSavedAt.value = null
    draftConfig = null
  }

  /** 加载成功后对比本地草稿与服务端配置：一致则静默丢弃，不一致才提示恢复 */
  function detectDraft(serverCfg: GatewayConfig) {
    const d = readDraft()
    if (!d) {
      hasDraft.value = false
      draftConfig = null
      return
    }
    try {
      if (JSON.stringify(d.config) === JSON.stringify(serverCfg)) {
        discardDraft()
        return
      }
    } catch {
      /* 比较失败时按「有草稿」处理，宁可多问一次也不静默丢用户输入 */
    }
    hasDraft.value = true
    draftSavedAt.value = d.savedAt
    draftConfig = d.config
  }

  // ===== 主操作：加载 / 保存 / 热重载 / 导出 / 预览变更 =====
  async function loadConfig() {
    try {
      const cfg = await apiExport()
      if (isUnmounted()) return
      originalToken.value = cfg.web?.api_token || ''
      Object.assign(config, cfg)
      if (!Array.isArray(config.buses)) config.buses = []
      if (!config.web) config.web = defaultWeb()
      if (!config.mqtt) config.mqtt = defaultMqtt()
      syncConfigToRaw()
      markClean()
      loadFailed.value = false
      detectDraft(cfg)
    } catch {
      // 修复(审查#14)：原实现 catch 直接吞掉，界面无任何"加载失败"提示，
      // 表单停留在一份默认配置上（看起来和加载成功没区别）。
      if (isUnmounted()) return
      // 基线同步为当前（默认）配置：加载失败后基线若停留在空串，
      // 之后任何一次改动都会被判脏，用户什么都没做就看到「未保存的更改」。
      markClean()
      loadFailed.value = true
      ElMessage.error('配置加载失败：当前显示的并非网关实际配置，请勿保存')
    }
  }

  async function saveConfig() {
    // 修复(审查#B7)：在飞保护——saving 为真时直接返回，避免双击叠加两次
    // confirm + postConfig + 热重载。整个流程用 try/finally 包裹，保证任意
    // 早返回 / 异常 / 卸载路径都会复位 saving。
    if (saving.value) return
    saving.value = true
    try {
      if (loadFailed.value) {
        // 修复(审查#14)：拦住"用默认配置覆盖网关真实配置"的破坏性保存
        ElMessage.warning('配置未成功加载，已禁止保存；请先重新加载配置')
        return
      }
      if (parseError.value) {
        ElMessage.warning('JSON 格式错误，请修正后再保存')
        return
      }
      // P4：校验规则在后端，需等待接口返回；请求失败时取消保存（避免无校验直发）。
      let issues: ConfigIssue[]
      try {
        issues = await validateConfig(config)
      } catch (e) {
        ElMessage.error(`${failureMessage('配置校验', e)}；已取消保存`)
        return
      }
      if (isUnmounted()) return
      const esc = (s: string) =>
        String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;')
      if (issues.length > 0) {
        const errMsgs = issues.filter((i: { level: string }) => i.level === 'error').map((i) => `· ${esc(i.message)}`)
        const warnMsgs = issues.filter((i: { level: string }) => i.level === 'warning').map((i) => `· ${esc(i.message)}`)
        const html = [...errMsgs, ...warnMsgs].join('<br>')
        if (errMsgs.length > 0) {
          ElMessageBox.alert(html, '配置校验未通过', { dangerouslyUseHTMLString: true, type: 'error' })
          return
        }
        // info 只是改进建议（完整性/性能提示），不阻断也不弹确认框
        if (warnMsgs.length > 0) {
          if (!(await confirmAction({
            message: `${html}<div style="margin-top:8px;color:var(--text-secondary)">存在警告，是否仍要保存？</div>`,
            title: '配置校验警告',
            confirmText: '仍要保存',
          }))) return
        }
      }
      if (!(await confirmAction({
        message: '保存配置将触发热重载，可能导致采集短暂中断。确认继续?',
        title: '保存配置',
        confirmText: '确认保存',
      }))) return
      if (isUnmounted()) return
      try {
        if (config.web) {
          if (!config.web.api_token) {
            config.web.api_token = originalToken.value
          } else if (config.web.api_token !== originalToken.value) {
            await setToken(config.web.api_token as string)
            originalToken.value = config.web.api_token as string
          }
        }
        await postConfig(config)
        if (isUnmounted()) return
        ElMessage.success('配置已保存并热重载')
        markClean()
        // 已真正落盘，本地草稿使命完成，清掉避免下次加载误提示
        discardDraft()
        useWebhook().notify('config_saved', '网关配置已保存并触发热重载').catch(() => { /* ignore */ })
        loadConfig()
      } catch (e) {
        // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示。
        // 保存失败的常见原因（配额超限 / 配置校验未过 / 磁盘只读）必须透出来
        ElMessage.error(`${failureMessage('保存配置', e)}；配置未被修改`)
      }
    } finally {
      if (!isUnmounted()) saving.value = false
    }
  }

  async function doReload() {
    if (!(await confirmAction({
      message: '将重新加载当前已保存的配置（不写入新配置），可能导致采集短暂中断。确认继续?',
      title: '热重载配置',
      type: 'info',
      confirmText: '确认重载',
    }))) return
    if (isUnmounted()) return
    reloading.value = true
    try {
      await reloadConfig()
      if (isUnmounted()) return
      ElMessage.success('配置已热重载')
      markClean()
      loadConfig()
    } catch (e) {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error(failureMessage('热重载配置', e))
    } finally {
      if (!isUnmounted()) reloading.value = false
    }
  }

  async function exportConfig() {
    try {
      const cfg = await apiExport()
      if (isUnmounted()) return
      const blob = new Blob([JSON.stringify(cfg, null, 2)], { type: 'application/json' })
      triggerDownload(blob, `gateway-config-${Date.now()}.json`)
      ElMessage.success('配置已导出（凭据字段已脱敏，迁移到新网关需重新填写）')
    } catch (e) {
      // 导出失败不能静默：用户会以为已下载，实际没有文件
      ElMessage.error(extractErrorMessage(e, '导出配置失败，请稍后重试'))
    }
  }

  function previewDiff() {
    showDiff.value = true
  }

  function onLocateIssue(tab: string) {
    activeTab.value = tab
  }

  return {
    // state
    activeTab,
    showValidate,
    showDiff,
    reloading,
    saving,
    config,
    gatewayConfig,
    originalToken,
    rawText,
    parseError,
    parseErrorMessage,
    tokenStatusText,
    rawJsonHtml,
    hasDraft,
    draftSavedAt,
    // actions
    loadConfig,
    saveConfig,
    doReload,
    exportConfig,
    previewDiff,
    addBus,
    removeBus,
    saveDraft,
    restoreDraft,
    discardDraft,
    syncConfigToRaw,
    resetRawText,
    onRawJsonInput,
    syncScroll,
    onLocateIssue,
  }
}