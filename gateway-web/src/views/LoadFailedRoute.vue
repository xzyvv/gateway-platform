<template>
  <!--
    路由层动态 import 失败的降级页
    ─────────────────────────────────
    Vue Router 默认对 dynamic import 失败不做兜底，导致用户看到空白页。
    本组件由 src/router/index.ts 的 router.onError 触发 router.replace 到此路由。
    复用 PageErrorBoundary 的"重试 + 复制错误"模式（独立于子组件，
    因此与侧栏/顶栏无关，能在子组件缺失/失败时也能正常渲染）。
  -->
  <div class="load-failed-page">
    <div class="load-failed-card">
      <div class="load-failed-header">
        <el-icon :size="40" color="var(--color-danger)"><WarningFilled /></el-icon>
        <h1>页面加载失败</h1>
        <p class="subtitle">无法加载当前页面的资源（可能是网络中断或版本错配）</p>
      </div>

      <details v-if="errorDetail" class="load-failed-detail">
        <summary>查看技术详情</summary>
        <pre>{{ errorDetail }}</pre>
      </details>

      <div class="load-failed-actions">
        <el-button type="primary" :loading="retrying" @click="retry">
          <el-icon><RefreshRight /></el-icon>
          <span>重试加载</span>
        </el-button>
        <el-button @click="goHome">
          <el-icon><HomeFilled /></el-icon>
          <span>返回首页</span>
        </el-button>
        <el-button text @click="reloadPage">
          <el-icon><Refresh /></el-icon>
          <span>刷新整页</span>
        </el-button>
        <el-button text @click="copyError" :title="copied ? '已复制' : '复制错误信息'">
          <el-icon><CopyDocument /></el-icon>
          <span>{{ copied ? '已复制' : '复制错误' }}</span>
        </el-button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import {
  WarningFilled, Refresh, RefreshRight, HomeFilled, CopyDocument,
} from '@element-plus/icons-vue'
import { reportPageError } from '../composables/core/useErrorReporter'
import { formatDateTime } from '@/utils/datetime'
// 整页跳转/刷新经 utils/navigation 间接一层：vmThreads 下 location 不可重定义，
// 测试需 vi.mock 本模块断言（见 LoadFailedRoute.spec.ts）
import { navigateTo, reloadPage as hardReload } from '@/utils/navigation'

/**
 * 路由 meta 扩展：error 字段携带原始错误对象 / message / 触发路径。
 * 由 router.onError 捕获 dynamic import 失败后 router.replace 写入。
 */
/** meta 字段定义在 src/types/router-meta.d.ts（loadError / loadErrorFrom） */

const route = useRoute()
const router = useRouter()

const errorText = computed(() => {
  return route.meta.loadError || '未知加载错误'
})

const fromPath = computed(() => {
  return route.meta.loadErrorFrom || (route.query.from as string | undefined) || ''
})

const errorDetail = computed(() => {
  const lines = [errorText.value]
  if (fromPath.value) lines.push(`来源路径：${fromPath.value}`)
  lines.push(`当前路径：${route.fullPath}`)
  lines.push(`时间：${formatDateTime(Date.now())}`)
  return lines.join('\n')
})

const retrying = ref(false)
const copied = ref(false)

/** 重试：清 meta.error，回到 fromPath 触发 Vue Router 重新 import */
async function retry() {
  retrying.value = true
  try {
    // 用整页跳转强制走完整 dynamic import 流程；
    // router.replace 在 chunk 已缓存失败的情况下不会重新触发 import 解析
    if (fromPath.value) {
      navigateTo(fromPath.value)
    } else {
      hardReload()
    }
  } finally {
    // 即使同步立即刷新，关闭 loading 状态避免长时间空白
    setTimeout(() => { retrying.value = false }, 1000)
  }
}

function goHome() {
  router.replace('/overview').catch(() => {
    // 若 /overview 也加载失败，强制刷新整页触发重试
    navigateTo('/')
  })
}

function reloadPage() {
  hardReload()
}

async function copyError() {
  try {
    await navigator.clipboard.writeText(errorDetail.value)
    copied.value = true
    ElMessage.success('错误信息已复制')
    setTimeout(() => { copied.value = false }, 2000)
  } catch {
    // 老浏览器降级：使用 execCommand
    try {
      const ta = document.createElement('textarea')
      ta.value = errorDetail.value
      ta.style.position = 'fixed'
      ta.style.opacity = '0'
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
      copied.value = true
      ElMessage.success('错误信息已复制')
      setTimeout(() => { copied.value = false }, 2000)
    } catch {
      ElMessage.warning('复制失败，请手动选择文本')
    }
  }
}

// 上报到统一的 error reporter（dashboard 错误指示器、聚合统计可见）
reportPageError({
  boundary: 'route-load',
  name: route.fullPath,
  error: errorText.value,
  route: route.fullPath,
  info: { from: fromPath.value, kind: 'dynamic-import' },
})
</script>

<style scoped>
.load-failed-page {
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  background:
    radial-gradient(ellipse at top right, rgba(var(--color-danger-rgb), 0.04) 0%, transparent 50%),
    linear-gradient(180deg, var(--bg-fill) 0%, var(--bg-fill) 100%);
  padding: 24px;
}

.load-failed-card {
  width: 100%;
  max-width: 520px;
  background: var(--bg-card);
  border-radius: 12px;
  padding: 36px 32px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.08);
}

.load-failed-header {
  text-align: center;
  margin-bottom: 24px;
}

.load-failed-header h1 {
  margin: 12px 0 6px;
  font-size: 1.375rem;
  font-weight: 600;
  color: var(--text-primary);
}

.load-failed-header .subtitle {
  margin: 0;
  color: var(--text-secondary);
  font-size: 0.875rem;
  line-height: 1.6;
}

.load-failed-detail {
  margin-bottom: 20px;
  border: 1px solid var(--el-border-color-lighter);
  border-radius: var(--radius-button);
  padding: 10px 14px;
  background: var(--el-fill-color-blank);
}

.load-failed-detail summary {
  cursor: pointer;
  color: var(--el-text-color-secondary);
  font-size: 0.8125rem;
  user-select: none;
}

.load-failed-detail pre {
  margin: 8px 0 0;
  padding: 10px;
  border-radius: var(--radius-small);
  background: var(--el-fill-color-light);
  font-family: var(--el-font-family-monospace, ui-monospace, monospace);
  font-size: 0.75rem;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--el-text-color-regular);
  max-height: 280px;
  overflow: auto;
}

.load-failed-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  justify-content: center;
}

/* 暗色补强 */
html.dark .load-failed-card {
  background: var(--bg-card);
  /* 暗色：纯黑阴影在深底上不可见，加载失败卡片与背景糊在一起 */
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
}
html.dark .load-failed-detail {
  background: rgba(255, 255, 255, 0.02);
  border-color: var(--el-border-color);
}
html.dark .load-failed-detail pre {
  background: rgba(0, 0, 0, 0.25);
  color: var(--el-text-color-secondary);
}
</style>