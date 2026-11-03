<template>
  <div class="rm-body">
    <div
      v-for="r in rows"
      :key="r.id"
      class="rm-row"
      :class="{ active: selectedId === r.id, error: isError(r) }"
      role="button"
      tabindex="0"
      @click="emit('select', r)"
      @keydown.enter.prevent="emit('select', r)"
      @keydown.space.prevent="emit('select', r)"
    >
      <span class="rm-method" :class="methodClass(r.method)">{{ r.method }}</span>
      <span class="rm-status" :class="statusClass(r)">{{ r.kind === 'ws' ? (r.ws?.direction === 'in' ? 'IN' : 'OUT') : r.status }}</span>
      <span class="rm-url">{{ r.url }}</span>
      <span class="rm-dur">{{ r.kind === 'ws' ? '—' : r.duration + 'ms' }}</span>
      <span class="rm-time">{{ formatTime(r.startTime) }}</span>
    </div>
    <EmptyState v-if="rows.length === 0" :description="emptyDescription" :image-size="60" />
  </div>
</template>

<script setup lang="ts">
import type { PropType } from 'vue'
import EmptyState from '../EmptyState.vue'
import type { RequestLogEntry as RequestRecord } from '@/stores/requestLog'

/**
 * 请求监视器的记录列表（纯展示 + 选中事件）。
 *
 * 抽成子组件只为控制父文件体积；根节点仍是 `div.rm-body`，类名与 DOM 层级
 * 与拆分前一致，因此父组件 flex 布局与既有单测（按 `.rm-row` 触发点击）都不受影响。
 */
defineProps({
  rows: { type: Array as PropType<RequestRecord[]>, required: true },
  /** 当前选中条目 id；详情由父组件渲染，这里只负责高亮 */
  selectedId: { type: Number, default: undefined },
  /** 空态文案（按「无记录 / 筛选无匹配」分情况，由父组件决定） */
  emptyDescription: { type: String, required: true },
})

const emit = defineEmits<{
  (e: 'select', row: RequestRecord): void
}>()

const isError = (r: RequestRecord) => typeof r.status === 'string' || r.status >= 400

const methodClass = (m: string) => {
  switch (m) {
    case 'GET': return 'm-get'
    case 'POST': return 'm-post'
    case 'PUT': return 'm-put'
    case 'DELETE': return 'm-del'
    case 'WS': return 'm-ws'
    default: return 'm-other'
  }
}

const statusClass = (r: RequestRecord) => {
  if (isError(r)) return 's-err'
  if (typeof r.status === 'number' && r.status >= 200 && r.status < 300) return 's-ok'
  return 's-other'
}

const formatTime = (ts: number) => {
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}:${String(d.getSeconds()).padStart(2, '0')}`
}
</script>

<style scoped>
.rm-body {
  flex: 1;
  overflow-y: auto;
  padding: 4px 0;
}
.rm-row {
  display: grid;
  grid-template-columns: 56px 60px 1fr 60px 64px;
  align-items: center;
  gap: 8px;
  padding: 6px 14px;
  cursor: pointer;
  border-bottom: 1px solid var(--bg-fill);
  font-family: 'Consolas', monospace;
  transition: background 0.15s;
}
.rm-row:hover { background: var(--color-primary-bg); }
.rm-row.active { background: var(--color-primary-bg); }
.rm-row.error { background: var(--color-danger-bg); }
.rm-row.error:hover { background: var(--el-color-danger-light-8); }
.rm-method { font-weight: 600; text-align: center; }
/* 语义色当字色时统一用 --text-* 变体：--color-success/-danger 在卡底上只有 2.2~2.9:1 */
.rm-method.m-get { color: var(--text-success); }
.rm-method.m-post { color: var(--color-primary); }
.rm-method.m-put { color: var(--text-warning); }
.rm-method.m-del { color: var(--text-danger); }
.rm-method.m-ws { color: var(--color-primary); }
.rm-method.m-other { color: var(--text-secondary); }
.rm-status { text-align: center; font-weight: 600; }
.rm-status.s-ok { color: var(--text-success); }
.rm-status.s-err { color: var(--text-danger); }
.rm-status.s-other { color: var(--text-secondary); }
.rm-url { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-primary); }
.rm-dur { text-align: right; color: var(--text-regular); }
.rm-time { text-align: right; color: var(--text-secondary); font-size: 0.75rem; }
</style>
