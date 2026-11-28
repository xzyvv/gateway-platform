<template>
  <!-- 悬浮触发按钮 -->
  <div
    v-if="!open && (isDev || store.enabled)"
    class="rm-fab"
    role="button"
    tabindex="0"
    @click="open = true"
    @keydown.enter.prevent="open = true"
    @keydown.space.prevent="open = true"
    :title="'请求监视器（' + total + '）'"
    :aria-label="'打开请求监视器，当前 ' + total + ' 条记录'"
  >
    <el-icon><Monitor /></el-icon>
    <span class="rm-fab-count" :class="{ bad: errors > 0 }">{{ total }}</span>
  </div>

  <!-- 展开面板：必须显式要求 open 为真（v-else-if），不能只靠上一个 v-if 的 else。
       生产环境 isDev=false 且关闭后 store.enabled=false，此时上个 v-if 为假，
       旧写法会把「面板」当成 else 分支渲染出来 → 面板永远关不掉。 -->
  <div v-else-if="open" class="rm-panel" :style="{ height: height + 'px' }">
    <div class="rm-header">
      <div class="rm-title">
        <el-icon><Monitor /></el-icon>
        <span>请求监视器</span>
      </div>
      <div class="rm-stats">
        <el-tag size="small" type="info">总数 {{ total }}</el-tag>
        <!-- 0 错误用中性色：success 标签在全站带「✓」底色图标（global.css 色弱辅助），
             出现在「错误 0」旁边会被读成「错误已通过」，语义相反 -->
        <el-tag size="small" :type="errors > 0 ? 'danger' : 'info'">错误 {{ errors }}</el-tag>
        <el-tag size="small" type="warning">均耗 {{ avgDuration }}ms</el-tag>
        <!-- 性能采样：FPS 反映主线程阻塞程度，JS Heap 反映 GC 压力 -->
        <el-tag size="small" :type="fpsTagType">
          {{ currentFps || '—' }} FPS
        </el-tag>
        <el-tag size="small" :type="memTagType">
          {{ currentMemPct !== null ? `${currentMemPct}% 内存` : '内存 N/A' }}
        </el-tag>
      </div>
      <div class="rm-actions">
        <el-button size="small" text @click="clear()">清空</el-button>
        <!-- 不再用 disabled：无记录时按钮点不动会让人以为坏了；改为点击给出明确提示 -->
        <el-button size="small" text title="导出为 HAR 文件（可拖进 Chrome DevTools 回放）" @click="exportHar">
          <el-icon><Download /></el-icon> 导出 HAR
        </el-button>
        <!-- 只在有选中记录时出现：没有详情可收起时这个按钮没有意义 -->
        <el-button v-if="selected" size="small" text title="收起下方详情面板" @click="selected = null">
          <el-icon><Close /></el-icon> 收起详情
        </el-button>
        <el-button size="small" text @click="paused = !paused">
          {{ paused ? '继续' : '暂停' }}
        </el-button>
        <el-button size="small" text aria-label="关闭请求监视器" title="关闭请求监视器" @click="open = false">
          <el-icon><Close /></el-icon>
        </el-button>
      </div>
    </div>

    <div class="rm-toolbar">
      <el-input
        v-model="filter"
        size="small"
        placeholder="过滤 URL / 方法 / WS..."
        aria-label="过滤请求"
        clearable
        style="width: 220px"
      />
      <el-radio-group v-model="statusFilter" size="small">
        <el-radio-button value="all">全部</el-radio-button>
        <el-radio-button value="error">仅错误</el-radio-button>
        <el-radio-button value="slow">慢请求(>1s)</el-radio-button>
      </el-radio-group>
      <div class="rm-hint" v-if="paused">已暂停记录</div>
    </div>

    <RequestRows
      :rows="filtered"
      :selected-id="selected?.id"
      :empty-description="emptyDesc"
      @select="selected = $event"
    />

    <RequestDetail
      v-if="selected"
      :record="selected"
      @close="selected = null"
      @copy-curl="copyAsCurl"
      @copy-diagnostic="copyDiagnostic"
    />

    <!-- 高度拖拽手柄 -->
    <div class="rm-resize" @mousedown="startResize"></div>
  </div>
</template>

<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { Close, Download, Monitor } from '@element-plus/icons-vue'
import { storeToRefs } from 'pinia'
import { useRequestLogStore, type RequestLogEntry as RequestRecord } from '@/stores/requestLog'
import { subscribeWsFrames } from '@/utils/websocket'
import { buildDiagnosticText } from '@/utils/diagnosticReport'
import { buildHar } from '@/utils/har'
import { triggerDownload } from '@/utils/exportCsv'
import { copyText } from '@/utils/clipboard'
import { isTauri } from '@/api/env'
import { getGatewayBaseUrl } from '@/api/gatewayConfig'
import { usePerfSampling } from '@/composables/core/usePerfSampling'
import RequestRows from './requestMonitor/RequestRows.vue'
import RequestDetail from './requestMonitor/RequestDetail.vue'

const store = useRequestLogStore()
const { logs, total, errors, avgDuration, paused } = storeToRefs(store)
const clear = store.clearLogs

// 面板显隐由外层（Layout 的用户菜单 / Ctrl+Shift+M）控制
const open = defineModel<boolean>('open', { default: false })
// 生产环境不常驻悬浮球：只在用户主动打开面板期间采样，关闭即停
const isDev = import.meta.env.DEV
const filter = ref('')
const statusFilter = ref<'all' | 'error' | 'slow'>('all')
const selected = ref<RequestRecord | null>(null)
const height = ref(420)

// 性能采样（FPS + JS Heap）细节见 composables/core/usePerfSampling.ts
const {
  currentFps,
  currentMemPct,
  fpsHistory,
  memHistory,
  startPerfSampling,
  stopPerfSampling,
  fpsTagType,
  memTagType,
} = usePerfSampling()

const isError = (r: RequestRecord) => typeof r.status === 'string' || r.status >= 400

// 空态文案按原因分：完全没记录 vs 筛选器筛掉了
const emptyDesc = computed(() => {
  if (logs.value.length === 0) return '还没有捕获到任何请求或 WS 帧，操作页面后稍候即可查看'
  if (filter.value) return `没有匹配「${filter.value}」的请求`
  if (statusFilter.value === 'error') return '没有错误请求 🎉'
  if (statusFilter.value === 'slow') return '没有慢于 1 秒的请求 🎉'
  return '没有请求记录'
})

// selected 持有的是日志条目引用，而日志上限 200 条、超出即 FIFO 淘汰。
// 三种情况下它会变成"悬空选中"：面板关闭后残留、条目被淘汰、清空后仍指向旧对象。
// 面板再次打开时会直接渲染一条列表里不存在的详情，用户会以为监视器出错。
//
// 依赖数组长度：logs 是 shallowRef，triggerRef 只发出「值变更」依赖且数组引用不变，
// watch(shallowRef) 默认浅比较不会触发；watch 数组长度既能覆盖 push/unshift，
// 也能覆盖 length 归零（clearLogs）。条目本身变化只关心"选中的还在不在列表里"。
watch(() => logs.value.length, () => {
  if (selected.value && !logs.value.some((r) => r.id === selected.value!.id)) {
    selected.value = null
  }
})
// 关闭面板即收起详情，并停止采样（开发环境保留常驻记录）
watch(open, (v) => {
  if (!v) selected.value = null
  store.setEnabled(v || isDev)
  // 面板显隐控制性能采样启停（采样开销虽小但没必要常驻）
  if (v || isDev) startPerfSampling()
  else stopPerfSampling()
})

// WS 帧也纳入监视：与 HTTP 共用面板/筛选/详情，便于判断「页面没刷新」是 WS 没推还是接口没调。
// pushLog 在面板关闭或暂停时于序列化前返回，所以常驻订阅不产生额外开销。
const unsubscribeWs = subscribeWsFrames((frame) => {
  const at = Date.now()
  store.pushLog({
    kind: 'ws',
    method: 'WS',
    url: frame.path,
    status: 101,
    duration: 0,
    startTime: at,
    endTime: at,
    ws: { direction: frame.direction, type: frame.type, size: frame.size, data: frame.data },
  })
})
onUnmounted(unsubscribeWs)

const filtered = computed(() => {
  let list = logs.value
  if (statusFilter.value === 'error') list = list.filter(isError)
  if (statusFilter.value === 'slow') list = list.filter((r) => r.duration > 1000)
  if (filter.value) {
    const kw = filter.value.toLowerCase()
    list = list.filter(
      (r) => r.url.toLowerCase().includes(kw) || r.method.toLowerCase().includes(kw),
    )
  }
  return list
})

/**
 * 一键复制诊断信息：时间 / 方法 / URL / 状态 / 错误码 / traceId / 错误原文。
 * 用户不需要理解 traceId 是什么，复制整段贴给研发即可对上后端日志。
 */
async function copyDiagnostic() {
  const r = selected.value
  if (!r) return
  if (r.kind === 'ws') {
    await copyText(
      buildDiagnosticText({ time: r.startTime, method: 'WS', url: r.url, status: r.ws?.type }),
      'WS 帧信息已复制',
    )
    return
  }
  await copyText(
    buildDiagnosticText({
      time: r.startTime,
      method: r.method,
      url: r.url,
      status: r.status,
      code: typeof r.status === 'number' ? r.status : undefined,
      duration: r.duration,
      traceId: r.traceId,
      error: r.error,
    }),
    '诊断信息已复制',
  )
}

/**
 * 复制为 cURL：便于在终端复现请求，或贴给研发在本地环境复现问题。
 *
 * 修复（P1-12）：原实现只输出「方法 + URL + Body」，并注明「请求头未记录」——
 * 于是带鉴权的接口在终端必然 401，HAR 里 `headers: []` 恒空也无法回放。
 * 现在把**实际记录到的请求头**一并输出：
 *   - 普通头（Content-Type / Accept 等）原样带上；
 *   - Authorization 因安全不落明文（记为 ***），这里以占位符形式给出，
 *     明确告诉用户「缺的是这一头、填自己的 token 即可」；
 *   - URL 补成绝对地址（Web 端用当前 origin，Tauri 端用网关地址），
 *     否则粘到终端请求的是相对路径，直接失败。
 */
async function copyAsCurl() {
  const r = selected.value
  if (!r || r.kind === 'ws') return
  const method = r.method.toUpperCase()
  // 日志里的 URL 是相对路径（/api/xxx），终端里必须绝对地址才能重放
  const base = isTauri() ? getGatewayBaseUrl() : window.location.origin
  const url = /^https?:\/\//i.test(r.url) ? r.url : `${base}${r.url}`
  const parts = [`curl -X ${method} '${url}'`]
  // 单引号转义：shell 里 ' 需写成 '\'' 才能安全嵌入
  const shQuote = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`
  let hasAuth = false
  for (const h of r.requestHeaders ?? []) {
    if (!h.name) continue
    if (h.name.toLowerCase() === 'authorization') {
      hasAuth = true
      // 故意不落明文 token：给出占位符 + 说明，让用户填自己的凭据
      parts.push(`-H 'Authorization: Bearer <你的 token>'`)
      continue
    }
    parts.push(`-H ${shQuote(`${h.name}: ${h.value}`)}`)
  }
  if (r.requestData !== undefined) {
    try {
      const body = typeof r.requestData === 'string' ? r.requestData : JSON.stringify(r.requestData)
      // 没有 Content-Type 头时补一个，否则服务端可能按表单解析 JSON
      if (!(r.requestHeaders ?? []).some((h) => h.name.toLowerCase() === 'content-type')) {
        parts.push(`-H 'Content-Type: application/json'`)
      }
      parts.push(`-d ${shQuote(body)}`)
    } catch { /* 序列化失败就不带 body */ }
  }
  // URL 里若有被脱敏的 query 参数（如 ?token=***），如实提示需要手工补
  if (url.includes('=***')) {
    parts.push(`# 注意：URL 中的凭据参数已被脱敏（***），请手工替换为真实值`)
  }
  if (hasAuth) {
    parts.push(`# 注意：Authorization 已脱敏，请把 <你的 token> 换成实际 token`)
  }
  await copyText(parts.join(' \\\n  '), 'cURL 命令已复制')
}

/**
 * 导出当前全部日志为 HAR 1.2（HTTP Archive），可拖进 Chrome DevTools Network 面板回放。
 * 构造逻辑与「一键诊断包」共用 src/utils/har.ts，保证两处导出口径一致。
 */
function exportHar() {
  if (logs.value.length === 0) {
    // 下载被浏览器/桌面壳拦截时界面上不会有任何变化，这里补一句可见反馈
    ElMessage.info('暂无可导出的请求记录')
    return
  }
  const blob = new Blob([buildHar(logs.value)], { type: 'application/json' })
  triggerDownload(blob, `gateway-requests-${Date.now()}.har`)
  ElMessage.success('HAR 已导出')
}

// 暂停时也允许查看，但不再接受新日志
// （记录逻辑在拦截器中，通过 paused 标志控制；这里仅做 UI 标记）

// 高度拖拽
let startY = 0
let startH = 0
const startResize = (e: MouseEvent) => {
  startY = e.clientY
  startH = height.value
  const onMove = (ev: MouseEvent) => {
    height.value = Math.max(240, Math.min(800, startH - (ev.clientY - startY)))
  }
  const onUp = () => {
    window.removeEventListener('mousemove', onMove)
    window.removeEventListener('mouseup', onUp)
  }
  window.addEventListener('mousemove', onMove)
  window.addEventListener('mouseup', onUp)
}

/* 测试可观测接口：性能采样历史数组 + 当前值，单元测试直接读 wrapper.vm 即可断言。
 * 同时暴露 startPerfSampling / stopPerfSampling 便于绕过 Vue watch 时机（fake timers 下可靠）。 */
defineExpose({
  fpsHistory,
  memHistory,
  currentFps,
  currentMemPct,
  startPerfSampling,
  stopPerfSampling,
})
</script>

<style scoped>
.rm-fab {
  position: fixed;
  right: 20px;
  bottom: 20px;
  z-index: 9999;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: linear-gradient(135deg, var(--color-primary), var(--color-accent));
  color: var(--text-inverse);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  box-shadow: 0 4px 16px rgba(var(--color-primary-rgb), 0.4);
  font-size: 1.25rem;
  transition: transform 0.2s;
}
.rm-fab:hover { transform: scale(1.08); }
.rm-fab-count {
  position: absolute;
  top: -4px;
  right: -4px;
  background: var(--color-info);
  color: var(--text-inverse);
  font-size: 0.625rem;
  min-width: 18px;
  height: 18px;
  border-radius: 9px;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0 4px;
  border: 2px solid var(--bg-card);
}
.rm-fab-count.bad { background: var(--color-danger); }

.rm-panel {
  position: fixed;
  right: 20px;
  bottom: 20px;
  z-index: 9999;
  width: 720px;
  max-width: calc(100vw - 40px);
  background: var(--bg-card);
  border-radius: var(--radius-card);
  box-shadow: var(--shadow-dialog);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  font-size: 0.8125rem;
}
/* 头部三块（标题 / 统计 / 操作）在空间不足时换行。
 * 之前是 nowrap：内容 680px 撑破 640px 面板后，右侧「导出 HAR / 暂停 / 关闭」被
 * .rm-panel 的 overflow:hidden 直接裁掉，导致面板关不掉、操作按钮点不到。 */
.rm-header {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 14px;
  flex-wrap: wrap;
  row-gap: 8px;
  background: linear-gradient(90deg, var(--color-primary), var(--color-accent));
  color: var(--text-inverse);
}
.rm-title { display: flex; align-items: center; gap: 6px; font-weight: 600; flex: 0 0 auto; }
.rm-stats { display: flex; gap: 6px; flex: 1 1 auto; min-width: 0; flex-wrap: wrap; }
.rm-stats .el-tag { --el-tag-bg-color: rgba(var(--text-inverse-rgb), 0.2); --el-tag-text-color: var(--text-inverse); border: none; }
.rm-actions { display: flex; gap: 4px; flex: 0 0 auto; margin-left: auto; flex-wrap: wrap; }
.rm-actions .el-button { color: var(--text-inverse); }

.rm-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 14px;
  border-bottom: 1px solid var(--border-base);
  background: var(--bg-card-header);
  /* 窄屏（max-width: 640px）时筛选控件换行，不横向溢出 */
  flex-wrap: wrap;
  row-gap: 8px;
}
.rm-hint { color: var(--text-warning); font-size: 0.75rem; font-weight: 500; }

.rm-resize {
  height: 6px;
  cursor: ns-resize;
  background: linear-gradient(180deg, transparent, rgba(0, 0, 0, 0.1));
}
</style>
