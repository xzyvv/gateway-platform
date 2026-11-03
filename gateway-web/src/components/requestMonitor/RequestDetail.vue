<template>
  <div class="rm-detail">
    <div class="rm-detail-header">
      <span>{{ record.kind === 'ws' ? 'WS 帧' : '请求详情' }} #{{ record.id }}</span>
      <div class="rm-detail-actions">
        <el-button
          v-if="record.kind !== 'ws'"
          size="small"
          text
          aria-label="复制为 cURL"
          title="复制为 cURL 命令，可在终端复现该请求"
          @click="emit('copy-curl')"
        >
          <el-icon><CopyDocument /></el-icon> cURL
        </el-button>
        <el-button
          size="small"
          text
          aria-label="复制诊断信息"
          title="复制诊断信息（含 traceId，可直接贴给研发）"
          @click="emit('copy-diagnostic')"
        >
          <el-icon><Document /></el-icon> 诊断
        </el-button>
        <el-button size="small" text aria-label="关闭详情" title="关闭详情" @click="emit('close')">
          <el-icon><Close /></el-icon>
        </el-button>
      </div>
    </div>
    <div class="rm-detail-body">
      <template v-if="record.kind === 'ws' && record.ws">
        <div class="rm-detail-row">
          <b>WS</b> {{ record.url }}
        </div>
        <div class="rm-detail-row">
          方向:
          <el-tag size="small" type="info">{{ record.ws.direction === 'in' ? '入站 IN' : '出站 OUT' }}</el-tag>
          类型: {{ record.ws.type }}
          长度: {{ record.ws.size }} 字符
        </div>
        <div class="rm-detail-label">帧内容</div>
        <pre class="rm-pre">{{ pretty(record.ws.data) }}</pre>
      </template>

      <template v-else>
        <div class="rm-detail-row">
          <b>{{ record.method }}</b> {{ record.url }}
        </div>
        <div class="rm-detail-row">
          状态: <el-tag size="small" :type="statusTagType">{{ record.status }}</el-tag>
          耗时: {{ record.duration }}ms
        </div>
        <div v-if="record.traceId" class="rm-detail-row">
          traceId: <span class="rm-trace">{{ record.traceId }}</span>
        </div>

        <!-- 请求头：原先完全不可见，用户既不知道发了什么头，也无法判断缺了哪一头 -->
        <template v-if="record.requestHeaders?.length">
          <div class="rm-detail-label">请求头 (Request Headers)</div>
          <pre class="rm-pre">{{ pretty(record.requestHeaders) }}</pre>
        </template>

        <template v-if="record.requestData">
          <div class="rm-detail-label">请求体 (Request)</div>
          <pre class="rm-pre">{{ pretty(record.requestData) }}</pre>
        </template>

        <template v-if="record.error">
          <div class="rm-detail-label">错误信息</div>
          <pre class="rm-pre rm-error">{{ record.error }}</pre>
        </template>

        <template v-else-if="record.responseData !== undefined">
          <div class="rm-detail-label">响应体 (Response)</div>
          <pre class="rm-pre">{{ pretty(record.responseData) }}</pre>
        </template>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, type PropType } from 'vue'
import { Close, CopyDocument, Document } from '@element-plus/icons-vue'
import type { RequestLogEntry as RequestRecord } from '@/stores/requestLog'

/**
 * 请求监视器的详情面板（受控展示组件）。
 *
 * 只负责渲染与「复制 / 关闭」事件上抛，复制内容如何拼装仍在父组件
 * （`copyAsCurl` / `copyDiagnostic`），避免把剪贴板与网关地址感知散进来。
 * 根节点仍是 `div.rm-detail`，类名与 CSS 从父组件原样迁入，视觉零变化。
 */
const props = defineProps({
  record: { type: Object as PropType<RequestRecord>, required: true },
})

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'copy-curl'): void
  (e: 'copy-diagnostic'): void
}>()

const isError = (r: RequestRecord) => typeof r.status === 'string' || r.status >= 400

const statusTagType = computed(() => (isError(props.record) ? 'danger' : 'success'))

const pretty = (v: unknown) => {
  try {
    return typeof v === 'string' ? v : JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}
</script>

<style scoped>
.rm-detail {
  border-top: 2px solid var(--color-primary);
  max-height: 45%;
  display: flex;
  flex-direction: column;
  background: var(--code-bg);
  color: var(--code-text);
}
.rm-detail-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 12px;
  background: var(--code-border-alt);
  font-weight: 600;
}
.rm-detail-header .el-button { color: var(--code-text); }
.rm-detail-actions { display: flex; align-items: center; gap: 2px; }
/* traceId 是排障凭据：等宽 + 单击全选，方便复制 */
.rm-trace {
  font-family: 'Consolas', monospace;
  color: var(--code-boolean);
  user-select: all;
  word-break: break-all;
}
.rm-detail-body {
  overflow-y: auto;
  padding: 10px 12px;
  font-family: 'Consolas', monospace;
  font-size: 0.75rem;
}
.rm-detail-row { margin-bottom: 6px; }
.rm-detail-row b { color: var(--code-boolean); }
.rm-detail-label {
  margin: 10px 0 4px;
  color: var(--code-key);
  font-weight: 600;
  font-size: 0.6875rem;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}
.rm-pre {
  margin: 0;
  padding: 8px;
  background: var(--code-panel);
  border-radius: var(--radius-small);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 240px;
  overflow: auto;
}
.rm-pre.rm-error { color: var(--code-danger); }
</style>
