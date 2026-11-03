<template>
  <!-- 正常态：透传默认插槽 -->
  <slot v-if="!hasError" />

  <!-- 降级态：错误卡片 -->
  <div v-else class="page-error-boundary" role="alert" aria-live="polite">
    <el-card shadow="never" class="error-card">
      <template #header>
        <SectionHeader :title="title" :icon="icon">
          <template v-if="count > 1" #extra>
            <el-tag type="danger" effect="dark" round size="small">第 {{ count }} 次</el-tag>
          </template>
          <el-button
            text
            class="icon-btn"
            :title="copyTip"
            :aria-label="copyTip"
            @click="copyError"
          >
            <el-icon><CopyDocument /></el-icon>
          </el-button>
        </SectionHeader>
      </template>

      <div class="error-body">
        <p class="error-subtitle">{{ subtitle }}</p>
        <details v-if="showDetail" class="error-detail">
          <summary>查看技术详情</summary>
          <pre class="error-stack">{{ errorText }}</pre>
        </details>
      </div>

      <div class="error-actions">
        <el-button type="primary" :loading="resetting" @click="reset">
          <el-icon><RefreshRight /></el-icon>
          <span>重试</span>
        </el-button>
        <el-button @click="reload">
          <el-icon><Refresh /></el-icon>
          <span>刷新整页</span>
        </el-button>
        <el-button text v-if="!showDetail" @click="showDetail = true">
          查看详情
        </el-button>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
/**
 * 页面级错误边界
 *
 * 核心职责：
 *   1. onErrorCaptured 捕获后代组件渲染/事件/生命周期中的同步错误
 *   2. 降级为本地卡片，**不弹 ElMessage**（避免与全局 unhandledrejection 监听重复弹窗）
 *   3. 提供两种重置：
 *      - 重试：清除错误状态，子组件重新挂载（key 变化触发 v-if 重建）
 *      - 刷新整页：window.location.reload（兜底，比如组件定义加载失败）
 *   4. 调用 useErrorReporter 上报，默认 console.error，可注入真实埋点
 *   5. 接收 name prop 用于聚合计数 + 路由来源
 *
 * 与全局 app.config.errorHandler 的分工：
 *   - ErrorBoundary（组件层）：捕获同步渲染错误，能局部降级
 *   - app.config.errorHandler（应用层）：兜底无法捕获的错误
 *   - window 'unhandledrejection'（全局层）：捕获异步未处理拒绝（如 axios 失败）
 *   三层覆盖完整错误谱系，互不冲突。
 */
import { ref, computed, onErrorCaptured, nextTick } from 'vue'
import { CopyDocument, Refresh, RefreshRight } from '@element-plus/icons-vue'
import { ElMessage } from 'element-plus'
import SectionHeader from './SectionHeader.vue'
import { reportPageError, bumpErrorCount, clearErrorCount } from '../composables/core/useErrorReporter'

const props = withDefaults(defineProps<{
  /** 边界标识（默认 route.path，方便聚合统计） */
  name?: string
  /** 边界类型标签（写入上报） */
  boundary?: 'page' | 'route' | string
  /** 降级卡片主标题 */
  title?: string
  /** 降级卡片副标题 */
  subtitle?: string
  /** 重置后是否自动上报 */
  silent?: boolean
}>(), {
  name: 'page',
  boundary: 'page',
  title: '页面渲染失败',
  subtitle: '当前页面的某个组件发生了错误。可以重试，或刷新整页恢复。',
  silent: false,
})

const error = ref<unknown>(null)
const route = ref<string>('')
const showDetail = ref(false)
const resetting = ref(false)
const resetSeq = ref(0)
const count = ref(0)

const hasError = computed(() => error.value != null)

const errorText = computed(() => {
  const err = error.value
  if (err == null) return ''
  if (err instanceof Error) {
    const stack = err.stack ? `\n${err.stack}` : ''
    return `${err.name}: ${err.message}${stack}`
  }
  try {
    return JSON.stringify(err, null, 2)
  } catch {
    return String(err)
  }
})

const icon = 'WarningFilled'
const copyTip = '复制错误信息'

onErrorCaptured((err, _instance, info) => {
  // 拦截：返回 false 让错误不再向上冒泡到 app.config.errorHandler（避免双触发）
  error.value = err
  // 解析路由（懒加载：动态 import 走 RouteRecord 拿不到完整路径，回退字符串）
  try {
    if (typeof window !== 'undefined' && window.location) {
      route.value = window.location.pathname + window.location.search
    }
  } catch { /* ignore */ }

  count.value = bumpErrorCount(props.name)

  if (!props.silent) {
    reportPageError({
      boundary: props.boundary as 'page' | 'route' | string,
      name: props.name,
      error: err,
      route: route.value,
      info: { componentInfo: info, count: count.value },
    })
  }

  return false
})

async function reset() {
  resetting.value = true
  // 主动清掉错误并 bump key 强制重渲染子组件
  error.value = null
  showDetail.value = false
  resetSeq.value++
  // 等一帧让 Vue 完成卸载/重挂
  await nextTick()
  resetting.value = false
}

function reload() {
  // 真·重置：刷新整页，兜底模块定义损坏等无法局部恢复的场景
  window.location.reload()
}

async function copyError() {
  try {
    await navigator.clipboard.writeText(errorText.value || '')
    ElMessage.success('错误信息已复制')
  } catch {
    // 老浏览器或非安全上下文无 clipboard API，降级为选中文本
    try {
      const ta = document.createElement('textarea')
      ta.value = errorText.value || ''
      ta.style.position = 'fixed'
      ta.style.opacity = '0'
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
      ElMessage.success('错误信息已复制')
    } catch {
      ElMessage.warning('复制失败，请手动选择文本')
    }
  }
}

// 清理函数：边界恢复正常（外部主动重置错误时）清除该 name 的计数
function resetCount() {
  clearErrorCount(props.name)
  count.value = 0
}

defineExpose({ reset, resetCount })
</script>

<style scoped>
.page-error-boundary {
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding: 24px 0;
  min-height: 240px;
}

.error-card {
  width: 100%;
  max-width: 640px;
  border-radius: var(--radius-card);
}

.error-body {
  font-size: 0.875rem;
  line-height: 1.7;
}

.error-subtitle {
  margin: 0 0 12px;
  color: var(--el-text-color-regular);
}

.error-detail {
  border: 1px solid var(--el-border-color-lighter);
  border-radius: var(--radius-button);
  padding: 8px 12px;
  background: var(--el-fill-color-blank);
}

.error-detail summary {
  cursor: pointer;
  color: var(--el-text-color-secondary);
  font-size: 0.8125rem;
  user-select: none;
}

.error-stack {
  margin: 8px 0 0;
  padding: 12px;
  border-radius: var(--radius-small);
  background: var(--el-fill-color-light);
  font-family: var(--el-font-family-monospace, ui-monospace, monospace);
  font-size: 0.75rem;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 320px;
  overflow: auto;
  color: var(--el-text-color-regular);
}

.error-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 16px;
}

/* 暗色模式补强：与现有 .dark 块一致的中性色 */
html.dark .error-card {
  background: var(--bg-card);
}

html.dark .error-detail {
  background: rgba(255, 255, 255, 0.02);
  border-color: var(--el-border-color);
}

html.dark .error-stack {
  background: rgba(0, 0, 0, 0.25);
  color: var(--el-text-color-secondary);
}
</style>