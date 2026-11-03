import { ref, computed, watch, type Ref } from 'vue'
import {
  defaultWeb,
  defaultMqtt,
  defaultSimConsumer,
  KNOWN_NULLABLE_KEYS,
  type ConfigForm,
} from '@/utils/configDefaults'

export interface UseConfigJsonOptions {
  /** 表单主数据（reactive，与 useConfigActions 共享同一实例） */
  config: ConfigForm
  /** 当前 tab（切换回 raw 时把 config 重新序列化到文本框） */
  activeTab: Ref<string>
  /** 检测到未识别顶层键时的提示回调（由调用方决定是否按挂载状态静默） */
  onUnknownTopKeys: (source: Record<string, unknown>, origin: string) => void
}

/**
 * 「原始 JSON」标签页的文本态与双向同步
 *
 * 把 rawText / parseError / 高亮 HTML 与 rawText ↔ config 的同步逻辑从
 * useConfigActions 抽出，仅负责文本侧的解析与序列化；配置节的兜底默认值
 * 与未知键提示通过注入的 config / 回调协作，避免与主 composable 相互依赖。
 */
export function useConfigJson({ config, activeTab, onUnknownTopKeys }: UseConfigJsonOptions) {
  const rawText = ref('')
  const parseError = ref(false)
  const parseErrorMessage = ref('')

  // ===== JSON 同步（rawText ↔ config）=====
  function syncRawToConfig() {
    try {
      const parsed = JSON.parse(rawText.value)
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        // 对象外的顶层形态直接拒绝（与 GatewayConfig 顶层约定 object 不符）
        throw new Error('顶层必须是 JSON 对象')
      }
      Object.keys(config).forEach((k) => delete (config as Record<string, unknown>)[k])
      Object.assign(config, parsed)
      // B2: 检测未识别顶层键，提示但不阻断
      onUnknownTopKeys(parsed, '原始 JSON')
      // 后端只接受数组形态的 buses，不做旧 map 形态兼容
      if (!Array.isArray(config.buses)) config.buses = []
      // web/mqtt 在后端为必填节（web: WebConfig | null 也是必填 Option，
      // 但前端 UI 长期默认带 web 配置，故保留兜底为 defaultWeb()）
      if (!config.web) config.web = defaultWeb()
      if (!config.mqtt) config.mqtt = defaultMqtt()
      // sim_consumer 在后端是带 serde 默认值的结构体（非 nullable）：解析的 JSON 漏写时
      // 会被上面的 delete+Object.assign 抹掉，这里补回默认值，避免整段配置回写后
      // 后端反序列化缺字段（虽然 serde 有 default，但前端类型要求该键必填）。
      if (!config.sim_consumer) config.sim_consumer = defaultSimConsumer()

      // B1: 其余 nullable 节（后端 serde 默认 null）若用户解析的 JSON 漏写，
      // Object.assign 不会写入，原值保留为 null 不算丢失；但「重置为最小骨架后
      // 用户先编辑再解析 JSON」的路径下，parsed 里缺这些键会保持上一轮状态——
      // 这里显式归一化为 null，避免历史脏值混入。
      for (const k of KNOWN_NULLABLE_KEYS) {
        if (!(k in parsed)) (config as Record<string, unknown>)[k] = null
      }
      // points 也是数组形态（无义）；缺省显式置 []
      if (!('points' in parsed)) (config as Record<string, unknown>).points = []
      // schema_version 必填；缺省置当前版本（与 GatewayConfig 构造种子一致）
      if (!('schema_version' in parsed)) (config as Record<string, unknown>).schema_version = 1

      parseError.value = false
      parseErrorMessage.value = ''
    } catch (e) {
      parseError.value = true
      parseErrorMessage.value = (e as Error)?.message || '未知错误'
    }
  }

  function syncConfigToRaw() {
    rawText.value = JSON.stringify(config, null, 2)
    parseError.value = false
    parseErrorMessage.value = ''
  }

  function onRawJsonInput() {
    syncRawToConfig()
  }

  // ===== JSON 高亮 =====
  const rawJsonHtml = computed(() => {
    let json = rawText.value
    json = json.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    json = json.replace(/"([^"\\]*(\\.[^"\\]*)*)"(\s*:)?/g, (_m, str, _esc, colon) => {
      const cls = colon ? 'json-key' : 'json-string'
      return `<span class="${cls}">"${str}"</span>${colon || ''}`
    })
    json = json.replace(/\b(\d+\.?\d*)\b/g, '<span class="json-number">$1</span>')
    json = json.replace(/\b(true|false)\b/g, '<span class="json-boolean">$1</span>')
    json = json.replace(/\b(null)\b/g, '<span class="json-null">$1</span>')
    return json
  })

  function syncScroll(e: Event) {
    const ta = e.target as HTMLTextAreaElement
    const pre = ta.previousElementSibling as HTMLElement
    if (pre) {
      pre.scrollTop = ta.scrollTop
      pre.scrollLeft = ta.scrollLeft
    }
  }

  // 切回 raw 标签时把 config 重新序列化到文本框（解析出错时保留用户输入不覆盖）
  watch(activeTab, (tab) => {
    if (tab === 'raw' && !parseError.value) {
      syncConfigToRaw()
    }
  })

  // 反向：raw 标签停留期间，表单字段变更同步回文本框
  watch(
    [() => config.buses, () => config.web, () => config.mqtt, () => config.ntp, () => config.telemetry, () => config.webhook],
    () => {
      if (activeTab.value === 'raw' && !parseError.value) {
        try {
          const currentRaw = rawText.value.trim()
          const generated = JSON.stringify(config, null, 2)
          if (currentRaw !== generated) {
            syncConfigToRaw()
          }
        } catch {
          /* 忽略不可序列化字段 */
        }
      }
    },
    { deep: true },
  )

  return {
    rawText,
    parseError,
    parseErrorMessage,
    rawJsonHtml,
    syncRawToConfig,
    syncConfigToRaw,
    onRawJsonInput,
    syncScroll,
  }
}