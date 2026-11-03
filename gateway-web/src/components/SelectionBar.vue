<template>
  <!-- 只在有勾选时出现：把"选中后能做什么"放在手边，而不是常驻一排灰按钮 -->
  <transition name="selection-bar">
    <div v-if="count > 0" class="selection-bar" role="status" aria-live="polite">
      <el-icon class="sb-icon"><Select /></el-icon>
      <span class="sb-text">
        已选 <strong>{{ count }}</strong> 项
        <span v-if="hint" class="sb-hint">{{ hint }}</span>
      </span>
      <div class="sb-actions">
        <slot />
      </div>
      <el-button link size="small" class="sb-clear" @click="emit('clear')">取消选择</el-button>
    </div>
  </transition>
</template>

<script setup lang="ts">
/**
 * 批量操作浮动条。
 *
 * 之前批量删除/批量修改常驻在页面工具栏，未选中时是灰色禁用——既占位置，
 * 又让人无法判断"这些按钮什么时候可用"。改成选中后才出现：
 *   1. 选择与动作在同一视觉区域，动作对象明确（这 N 项）
 *   2. 跨页勾选时给出提示，避免用户以为只作用于当前页
 *   3. 提供"取消选择"一键退出，而不是逐行取消
 *
 * 位置放在表格正上方而不是固定悬浮：表格自身是内部滚动 + 固定表头，
 * 条本身不会随行滚出视野，既保持可见又不会遮挡内容。
 */
import { Select } from '@element-plus/icons-vue'

withDefaults(defineProps<{
  /** 已选行数；为 0 时整个条不渲染 */
  count: number
  /** 选择范围提示，例如"可跨页勾选" */
  hint?: string
}>(), { hint: '' })

const emit = defineEmits<{ clear: [] }>()
</script>

<style scoped>
.selection-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
  padding: 8px 12px;
  border: 1px solid var(--color-primary);
  border-radius: var(--radius-table);
  background: var(--color-primary-lightest);
  box-shadow: 0 2px 8px var(--color-primary-shadow);
}

.sb-icon {
  font-size: 1rem;
  color: var(--color-primary);
  flex-shrink: 0;
}

.sb-text {
  font-size: 0.8125rem;
  color: var(--text-primary);
}
.sb-text strong {
  font-variant-numeric: tabular-nums;
  color: var(--color-primary);
}
.sb-hint {
  margin-left: 6px;
  font-size: 0.75rem;
  color: var(--text-secondary);
}

.sb-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-left: auto;
}

.sb-clear {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.selection-bar-enter-active,
.selection-bar-leave-active {
  transition: opacity 0.16s ease, transform 0.16s ease;
}
.selection-bar-enter-from,
.selection-bar-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

@media (prefers-reduced-motion: reduce) {
  .selection-bar-enter-active,
  .selection-bar-leave-active {
    transition: none;
  }
}
</style>