<template>
  <!--
    访问控制 / 路径兜底页（顶层路由，全屏渲染，不套侧栏与顶栏）
    ──────────────────────────────────────────────────
    存在意义：把两种此前都被「静默跳回仪表盘」的失败区分开——
      · forbidden：已登录但当前账号无权访问该页（admin 守卫跳转至此）
      · not-found：地址打错或页面已下线（catch-all 跳转至此）
    技术支持远程排查时，能一眼看出是权限问题还是路径问题，
    而不是只看到一闪而过的 toast 后莫名其妙落在仪表盘。
  -->
  <div class="fallback-page">
    <div class="fallback-card">
      <div class="fallback-header">
        <el-icon :size="40" :color="kind === 'forbidden' ? 'var(--color-warning)' : 'var(--color-info)'">
          <Lock v-if="kind === 'forbidden'" />
          <QuestionFilled v-else />
        </el-icon>
        <h1>{{ title }}</h1>
        <p class="subtitle">{{ subtitle }}</p>
      </div>

      <details class="fallback-detail">
        <summary>查看详情</summary>
        <pre>{{ detail }}</pre>
      </details>

      <div class="fallback-actions">
        <el-button type="primary" @click="goHome">
          <el-icon><HomeFilled /></el-icon>
          <span>返回首页</span>
        </el-button>
        <el-button v-if="kind === 'forbidden'" @click="switchAccount">
          <el-icon><SwitchButton /></el-icon>
          <span>切换账号</span>
        </el-button>
        <el-button text @click="copyDetail" :title="copied ? '已复制' : '复制诊断信息'">
          <el-icon><CopyDocument /></el-icon>
          <span>{{ copied ? '已复制' : '复制信息' }}</span>
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
  Lock, QuestionFilled, HomeFilled, CopyDocument, SwitchButton,
} from '@element-plus/icons-vue'
import { useAuthStore } from '../stores/auth'
import { formatDateTime } from '@/utils/datetime'

const route = useRoute()
const router = useRouter()
const auth = useAuthStore()

/** 由路由 meta.fallbackKind 决定展示哪种失败；缺省按 not-found 处理 */
const kind = computed<'forbidden' | 'not-found'>(() =>
  route.meta.fallbackKind === 'forbidden' ? 'forbidden' : 'not-found')

/** 触发本次跳转的原始路径（由守卫 / catch-all 写入 query.from） */
const fromPath = computed(() => (route.query.from ? String(route.query.from) : ''))

const title = computed(() => (kind.value === 'forbidden' ? '无权访问该页面' : '页面不存在'))

const subtitle = computed(() =>
  kind.value === 'forbidden'
    ? '当前账号没有访问该页面的权限。若确实需要使用，请联系管理员调整账号角色。'
    : '你访问的地址不存在，可能是链接输入有误，或该页面已被移除。')

const copied = ref(false)

const detail = computed(() => {
  const lines: string[] = ['类型：' + (kind.value === 'forbidden' ? '无权限访问' : '页面不存在')]
  if (fromPath.value) lines.push(`访问路径：${fromPath.value}`)
  if (auth.username) lines.push(`当前账号：${auth.username}（角色 ${auth.roleId || '未知'}）`)
  lines.push(`时间：${formatDateTime(Date.now())}`)
  return lines.join('\n')
})

function goHome() {
  router.replace('/overview')
}

function switchAccount() {
  auth.logout()
  router.replace('/login')
}

/** 远程排查场景需要把路径/账号信息发给研发，直接给文本而非让人手抄 */
async function copyDetail() {
  try {
    await navigator.clipboard.writeText(detail.value)
    copied.value = true
    ElMessage.success('信息已复制')
    setTimeout(() => { copied.value = false }, 2000)
  } catch {
    ElMessage.warning('复制失败，请展开详情手动选择文本')
  }
}
</script>

<style scoped>
.fallback-page {
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  background: linear-gradient(180deg, var(--bg-fill) 0%, var(--bg-fill) 100%);
  padding: 24px;
}

.fallback-card {
  width: 100%;
  max-width: 520px;
  background: var(--bg-card);
  border-radius: 12px;
  padding: 36px 32px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.08);
}

.fallback-header {
  text-align: center;
  margin-bottom: 24px;
}

.fallback-header h1 {
  margin: 12px 0 6px;
  font-size: 1.375rem;
  font-weight: 600;
  color: var(--text-primary);
}

.fallback-header .subtitle {
  margin: 0;
  color: var(--text-secondary);
  font-size: 0.875rem;
  line-height: 1.6;
}

.fallback-detail {
  margin-bottom: 20px;
  border: 1px solid var(--el-border-color-lighter);
  border-radius: var(--radius-button);
  padding: 10px 14px;
  background: var(--el-fill-color-blank);
}

.fallback-detail summary {
  cursor: pointer;
  color: var(--el-text-color-secondary);
  font-size: 0.8125rem;
  user-select: none;
}

.fallback-detail pre {
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

.fallback-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  justify-content: center;
}

/* 暗色补强 */
html.dark .fallback-card {
  /* 暗色：纯黑阴影在深底上不可见，错误页卡片与背景糊在一起 */
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
}
html.dark .fallback-detail {
  background: rgba(255, 255, 255, 0.02);
  border-color: var(--el-border-color);
}

html.dark .fallback-detail pre {
  background: rgba(0, 0, 0, 0.25);
  color: var(--el-text-color-secondary);
}
</style>
