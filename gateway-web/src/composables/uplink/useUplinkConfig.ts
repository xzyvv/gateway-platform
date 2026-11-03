import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import { ElMessage, ElNotification } from 'element-plus'
import {
  getUplinkConfig,
  saveUplinkConfig,
  listTemplateVars,
  exportTemplate as exportTemplateApi,
  importTemplate as importTemplateApi,
  BUILTIN_TEMPLATES,
  type UplinkConfig,
  type TemplateVarInfo,
} from '@/api/uplink'
import { logWarn } from '@/utils/logger'
import { registerDirtyGuard } from '@/utils/dirtyGuard'
import { triggerDownload } from '@/utils/exportCsv'

/**
 * UplinkConfig 页面数据层封装
 *
 * 把 UplinkConfig.vue 中的配置状态、变量清单、模板编译 computed、保存/加载/
 * 导出/导入/应用模板/插入变量、跳转调试工具、生命周期全部封装，主组件
 * 只剩 template。
 *
 * 职责边界：本页只负责「通道级配置的持久化」；模板渲染预览与真实上行测试
 * 统一由「上行测试」页（UplinkTester.vue）承担，避免两处实现同一件事。
 *
 * 命名说明：已统一为 useUplinkConfig（去掉 View 后缀）。
 */
export function useUplinkConfig() {
  const router = useRouter()
  const route = useRoute()
  const loading = ref(false)
  const saving = ref(false)
  const exporting = ref(false)
  const importInput = ref<HTMLInputElement | null>(null)

  const cfg = ref<UplinkConfig>({
    enabled: false,
    template: BUILTIN_TEMPLATES[0]!.template,
    encoding: 'json',
    max_bytes: 4096,
  })

  // 未保存更改守卫：内联表单，修改即标脏；保存/重载成功后重置；卸载时注销。
  // scope 用 route.path（/uplink），与 HubShell / router.beforeEach 的 confirmDiscardDirty 对齐。
  const dirtyGuard = registerDirtyGuard(route.path)
  let dirtyBaseline = JSON.stringify(cfg.value)
  function markClean() {
    dirtyBaseline = JSON.stringify(cfg.value)
    dirtyGuard.reset()
  }
  watch(() => JSON.stringify(cfg.value), (s) => {
    if (s === dirtyBaseline) dirtyGuard.reset()
    else dirtyGuard.mark()
  })
  onUnmounted(() => dirtyGuard.unregister())

  const templateVars = ref<TemplateVarInfo[]>([])
  const varsPanelOpen = ref<string[]>([])
  const selectedTemplateId = ref<string>('')

  const compileError = computed<string | null>(() => {
    if (!cfg.value.template) return null
    const opens = (cfg.value.template.match(/\$\{/g) || []).length
    const closes = (cfg.value.template.match(/\}/g) || []).length
    if (opens > closes) return `存在未闭合的 \${ (${opens} 个开始、${closes} 个结束)`
    return null
  })

  const templateBytes = computed(() => new Blob([cfg.value.template]).size)
  const variableCount = computed(() => (cfg.value.template.match(/\$\{[^}]+\}/g) || []).length)
  const totalVariables = computed(() => templateVars.value.length)

  function applyTemplate(id: string) {
    const tpl = BUILTIN_TEMPLATES.find((t) => t.id === id)
    if (tpl) {
      cfg.value.template = tpl.template
      ElMessage.success(`已应用模板：${tpl.name}`)
    }
  }

  function insertVar(name: string) {
    cfg.value.template = `${cfg.value.template}${cfg.value.template.endsWith('\n') ? '' : '\n'}\${${name}}`
    ElMessage.success(`已追加 \${${name}}`)
  }

  async function loadConfig() {
    loading.value = true
    try {
      cfg.value = await getUplinkConfig()
      markClean()
    } catch (e) {
      // 首次访问可能 404，使用默认配置
      logWarn('uplink', 'load config failed (using defaults):', e)
    } finally {
      loading.value = false
    }
  }

  async function saveConfig() {
    if (compileError.value) {
      ElMessage.error('模板编译失败，无法保存')
      return
    }
    saving.value = true
    try {
      const saved = await saveUplinkConfig(cfg.value)
      cfg.value = saved
      markClean()
      ElNotification.success({
        title: '已保存',
        message: '上行模板配置已生效，下次发包起使用新格式',
        duration: 2500,
      })
    } catch {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error('保存上行模板配置失败，请稍后重试')
    } finally {
      saving.value = false
    }
  }

  async function doExportTemplate() {
    exporting.value = true
    try {
      const data = await exportTemplateApi()
      const payload = JSON.stringify({
        template: data.template,
        enabled: data.enabled,
        encoding: data.encoding,
        max_bytes: data.max_bytes,
        exported_at: data.exported_at,
      }, null, 2)
      const blob = new Blob([payload], { type: 'application/json' })
      triggerDownload(blob, `uplink-template-${Date.now()}.json`)
      ElMessage.success('模板已导出为 JSON 文件')
    } catch {
      /* axios 拦截器已弹窗 */
    } finally {
      exporting.value = false
    }
  }

  function triggerImport() {
    importInput.value?.click()
  }

  async function handleImport(e: Event) {
    const target = e.target as HTMLInputElement
    const file = target.files?.[0]
    if (!file) return
    try {
      const text = await file.text()
      const data = JSON.parse(text) as { template?: string; enabled?: boolean }
      if (!data.template || typeof data.template !== 'string') {
        ElMessage.warning('文件格式不正确：缺少 template 字符串字段')
        return
      }
      await importTemplateApi({
        template: data.template,
        enabled: data.enabled ?? true,
      })
      ElNotification.success({
        title: '导入成功',
        message: '模板已应用并热重载，下次发包起使用新格式',
        duration: 2500,
      })
      await loadConfig()
    } catch (err) {
      if (err instanceof SyntaxError) {
        ElMessage.warning('JSON 解析失败：' + err.message)
      } else {
        // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
        ElMessage.error('导入模板失败，请稍后重试')
      }
    } finally {
      target.value = ''
    }
  }

  function goTester() {
    router.push('/uplink?tab=uplink-tester')
  }

  onMounted(async () => {
    await loadConfig()
    try {
      templateVars.value = await listTemplateVars()
    } catch (err) {
      // 变量清单以 /api/uplink/vars 为准；取不到时保持为空，避免展示与后端不一致的硬编码清单
      logWarn('uplink:vars', err, '变量清单加载失败')
    }
  })

  return {
    // state
    loading, saving, exporting, importInput,
    templateVars, varsPanelOpen, selectedTemplateId, cfg,
    // computed
    compileError, templateBytes, variableCount, totalVariables,
    // constants
    BUILTIN_TEMPLATES,
    // actions
    loadConfig, saveConfig,
    doExportTemplate, triggerImport, handleImport,
    applyTemplate, insertVar, goTester,
  }
}
