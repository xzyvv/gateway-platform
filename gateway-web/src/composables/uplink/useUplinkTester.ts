import { ref, computed, reactive, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import {
  testUplink,
  getUplinkConfig,
  listTestHistory,
  clearTestHistory,
  BUILTIN_TEMPLATES,
  type UplinkTestResponse,
  type UplinkTestHistoryItem,
} from '@/api/uplink'

interface TesterState {
  template: string
  vars: Record<string, string | number | boolean>
}

const RANDOM_SAMPLES = [
  { device_id: '电表1', device_group: '车间A', point_id: 'voltage_a', value: 220 + Math.random() * 10, unit: 'V' },
  { device_id: 'PLC_03', device_group: '产线2', point_id: 'motor_speed', value: 1450 + Math.random() * 50, unit: 'rpm' },
  { device_id: '水表_001', device_group: '车间B', point_id: 'flow_total', value: 12345.67, unit: 'm³' },
  { device_id: '温湿度_01', device_group: '车间A', point_id: 'temperature', value: 25 + Math.random() * 5, unit: '℃' },
]

/**
 * UplinkTester 页面数据层封装
 *
 * 把 UplinkTester.vue 中的模板编辑、变量输入、前端兜底渲染、编译校验、
 * 测试发送、历史加载/清空/复用、跳转配置页集中到一个 composable，主组件只剩 template。
 */
export function useUplinkTester() {
  const router = useRouter()
  const testing = ref(false)
  const sending = ref(false)
  const lastResult = ref<UplinkTestResponse | null>(null)
  const history = ref<UplinkTestHistoryItem[]>([])
  const historyLoading = ref(false)

  const state = reactive<TesterState>({
    template: BUILTIN_TEMPLATES[0]!.template,
    vars: {
      device_id: '电表1',
      device_group: '车间A',
      point_id: 'voltage_a',
      unit: 'V',
      quality_text: 'good',
    },
  })

  const valueNum = computed({
    get: () => Number(state.vars.value ?? 0),
    set: (v: number) => { state.vars.value = Number.isFinite(v) ? v : 0 },
  })
  const staleNum = computed({
    get: () => Number(state.vars.stale_count ?? 0),
    set: (v: number) => { state.vars.stale_count = Math.max(0, Math.floor(v)) },
  })

  /** 前端兜底渲染（与 Rust 端一致语义：裸值输出） */
  function renderFrontend(): string {
    const tpl = state.template
    if (!tpl) return ''
    return tpl.replace(/\$\{([^}]+)\}/g, (_match: string, name: string) => {
      const v = state.vars[name.trim()]
      if (v === undefined || v === null) return ''
      if (typeof v === 'number') {
        if (!Number.isFinite(v)) return 'null'
        if (Number.isInteger(v)) return String(v)
        const s = String(v)
        return s.includes('.') || s.includes('e') ? s : `${s}.0`
      }
      if (typeof v === 'boolean') return v ? 'true' : 'false'
      return String(v)
    })
  }

  const rendered = computed(() => renderFrontend())
  const templateBytes = computed(() => new Blob([state.template]).size)
  const renderedBytes = computed(() => new Blob([rendered.value]).size)

  const compileError = computed<string | null>(() => {
    if (!state.template) return null
    const opens = (state.template.match(/\$\{/g) || []).length
    const closes = (state.template.match(/\}/g) || []).length
    if (opens > closes) return `存在未闭合的 \${ （${opens} 个开始 / ${closes} 个结束）`
    return null
  })

  function applyTemplate(id: string) {
    const tpl = BUILTIN_TEMPLATES.find((t) => t.id === id)
    if (tpl) {
      state.template = tpl.template
      ElMessage.success(`已应用：${tpl.name}`)
    }
  }

  function randomizeData() {
    const sample = RANDOM_SAMPLES[Math.floor(Math.random() * RANDOM_SAMPLES.length)]
    Object.assign(state.vars, sample)
    ElMessage.success('已填入随机示例数据')
  }

  async function loadFromConfig() {
    try {
      const cfg = await getUplinkConfig()
      state.template = cfg.template || state.template
      ElMessage.success('已加载保存的模板')
    } catch {
      ElMessage.warning('后端未提供 /api/uplink/config，使用本地默认')
    }
  }

  function goConfig() {
    // W2-#13：模板配置已并入「上行配置」页，旧 uplink-config 页签已下线
    router.push('/uplink?tab=forward-config')
  }

  async function copyRendered() {
    try {
      await navigator.clipboard.writeText(rendered.value)
      ElMessage.success('已复制到剪贴板')
    } catch {
      ElMessage.warning('剪贴板访问失败，请手动复制')
    }
  }

  async function loadHistory() {
    historyLoading.value = true
    try {
      history.value = await listTestHistory(50)
    } catch {
      /* axios 拦截器已弹窗 */
    } finally {
      historyLoading.value = false
    }
  }

  async function confirmClearHistory() {
    if (!(await confirmDelete('全部测试历史记录', { title: '清空测试历史', confirmText: '清空' }))) return
    try {
      await clearTestHistory()
      history.value = []
      ElMessage.success('测试历史已清空')
    } catch {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error('清空测试历史失败，请稍后重试')
    }
  }

  function reuseHistory(row: UplinkTestHistoryItem) {
    state.template = row.template
    try {
      const vars = JSON.parse(row.vars_json || '{}') as Record<string, string | number | boolean>
      Object.assign(state.vars, vars)
      ElMessage.success('已复用历史记录的模板与变量')
    } catch {
      ElMessage.warning('历史变量解析失败，仅复用模板')
    }
  }

  async function testSend(send: boolean) {
    const action = send ? sending : testing
    action.value = true
    try {
      const resp = await testUplink({
        template: state.template,
        vars: state.vars,
        send,
      })
      lastResult.value = resp

      if (resp.ok) {
        const msg = send
          ? (resp.publish?.delivered ? `已送达 ${resp.publish.topic}` : '渲染成功但未送达')
          : '渲染成功'
        if (msg.startsWith('已送达') || msg === '渲染成功') {
          ElMessage.success(msg)
        } else {
          ElMessage.warning(msg)
        }
      } else {
        ElMessage.error(resp.error || '失败')
      }

      if (send) {
        await loadHistory()
      }
    } catch {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error('上行测试请求失败，请稍后重试')
    } finally {
      action.value = false
    }
  }

  function formatTime(t: number): string {
    const d = new Date(t)
    const pad = (n: number) => String(n).padStart(2, '0')
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  }

  onMounted(async () => {
    randomizeData()
    await loadHistory()
  })

  return {
    // state
    testing, sending, lastResult, history, historyLoading,
    state, valueNum, staleNum,
    // computed
    rendered, templateBytes, renderedBytes, compileError,
    // constants
    BUILTIN_TEMPLATES,
    // actions
    applyTemplate, randomizeData, loadFromConfig, goConfig,
    copyRendered, loadHistory, confirmClearHistory, reuseHistory,
    testSend, formatTime,
  }
}
