<template>
  <div class="inline-edit" :class="[`is-${align}`, { 'is-mono': mono, 'is-editing': editing }]">
    <!-- 展示态：双击进入编辑，也支持键盘 Enter/F2，避免只能靠鼠标发现 -->
    <span
      v-if="!editing"
      ref="viewRef"
      class="iec-view"
      :class="{ 'is-empty': isEmpty, 'is-disabled': disabled }"
      :title="disabled ? '' : `双击修改${label}`"
      :tabindex="disabled ? -1 : 0"
      role="button"
      :aria-label="`${label}：${displayText}${disabled ? '' : '，双击可修改'}`"
      @dblclick="startEdit"
      @keydown.enter.prevent="startEdit"
      @keydown.f2.prevent="startEdit"
    >{{ displayText }}</span>

    <!-- 编辑态：Enter 保存、Esc 取消；失焦视为放弃，避免误触写配置 -->
    <span v-else class="iec-edit">
      <el-input
        ref="inputRef"
        v-model="draft"
        size="small"
        :maxlength="maxlength"
        :disabled="saving"
        :aria-label="label"
        @keydown.enter.prevent="commit"
        @keydown.esc.prevent="cancel(true)"
        @blur="cancel()"
      />
      <span v-if="saving" class="iec-saving">保存中…</span>
    </span>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { ElInput, ElMessage } from 'element-plus'

const props = withDefaults(
  defineProps<{
    value?: string | number | null
    /** 列名，用于提示与无障碍播报 */
    label: string
    type?: 'text' | 'number'
    min?: number
    max?: number
    maxlength?: number
    /** 文本类字段是否允许留空 */
    required?: boolean
    mono?: boolean
    align?: 'left' | 'right'
    disabled?: boolean
    /** 提交回调：返回 false 表示保存失败，需保留用户输入 */
    save: (value: string | number) => Promise<boolean>
  }>(),
  {
    value: '',
    type: 'text',
    min: 0,
    max: 9999,
    maxlength: 64,
    required: true,
    align: 'left',
    disabled: false,
  },
)

const editing = ref(false)
const saving = ref(false)
const draft = ref('')
const inputRef = ref<{ focus: () => void } | null>(null)
const viewRef = ref<{ focus: () => void } | null>(null)

const isEmpty = computed(() => props.value == null || String(props.value).trim() === '')
const displayText = computed(() => (isEmpty.value ? '—' : String(props.value)))

/** 退出编辑态后把焦点交还给展示态单元格，键盘用户不会"掉到页面顶部" */
function focusView() {
  void nextTick(() => viewRef.value?.focus())
}

function startEdit() {
  if (props.disabled || saving.value || editing.value) return
  // 数字也先按字符串编辑，提交时再校验，避免输入过程中被 input-number 强行纠正
  draft.value = isEmpty.value ? '' : String(props.value)
  editing.value = true
  nextTick(() => inputRef.value?.focus())
}

/**
 * 退出编辑态。
 * @param restoreFocus 是否把焦点还给展示态单元格。
 *   Esc 取消 → true（焦点跟着用户意图走）；失焦取消 → false，
 *   否则会把用户正在点击的其他控件的焦点抢回来。
 */
function cancel(restoreFocus = false) {
  if (!editing.value || saving.value) return
  editing.value = false
  draft.value = ''
  if (restoreFocus) focusView()
}

/** 校验并归一化；返回 undefined 表示输入非法（保持编辑态让用户改） */
function normalize(): string | number | undefined {
  const raw = draft.value.trim()
  if (props.type === 'number') {
    if (!/^-?\d+$/.test(raw)) {
      ElMessage.warning(`${props.label}需为整数`)
      return undefined
    }
    const n = Number(raw)
    if (n < props.min || n > props.max) {
      ElMessage.warning(`${props.label}需在 ${props.min}–${props.max} 之间`)
      return undefined
    }
    return n
  }
  if (!raw && props.required) {
    ElMessage.warning(`${props.label}不能为空`)
    return undefined
  }
  return raw
}

async function commit() {
  if (!editing.value || saving.value) return
  const next = normalize()
  if (next === undefined) return
  // 值没变就不发请求，避免无意义的配置写入
  if (next === (isEmpty.value ? '' : props.value)) {
    editing.value = false
    focusView()
    return
  }
  saving.value = true
  let ok = false
  try {
    ok = await props.save(next)
  } catch {
    // save 自身抛错按失败处理：保持编辑态，用户输入不丢
  } finally {
    saving.value = false
  }
  if (ok) {
    editing.value = false
    draft.value = ''
    focusView()
  }
}
</script>

<style scoped>
.inline-edit {
  width: 100%;
  min-width: 0;
}
.inline-edit.is-right {
  text-align: right;
}
.inline-edit.is-mono .iec-view,
.inline-edit.is-mono :deep(.el-input__inner) {
  font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace);
}

.iec-view {
  display: inline-block;
  max-width: 100%;
  padding: 0 2px;
  border-radius: 3px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
  cursor: text;
  /* 常驻虚线下划线会让表格显得嘈杂，只在悬停时提示"这里可编辑" */
  border-bottom: 1px dashed transparent;
  transition: background-color 0.15s, border-color 0.15s;
}
.iec-view:hover {
  background: var(--bg-fill);
  border-bottom-color: var(--border-base);
}
/* 键盘焦点必须可见：仅靠背景色变化在白底上只有 1.07:1，键盘用户根本看不到焦点在哪 */
.iec-view:focus-visible {
  background: var(--bg-fill);
  border-bottom-color: var(--border-base);
  outline: 2px solid var(--color-primary);
  outline-offset: 2px;
}
.iec-view.is-empty {
  color: var(--text-placeholder);
}
.iec-view.is-disabled {
  cursor: default;
}
.iec-view.is-disabled:hover {
  background: transparent;
  border-bottom-color: transparent;
}

.iec-edit {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  width: 100%;
}
.iec-edit :deep(.el-input) {
  width: 100%;
}
.iec-saving {
  flex-shrink: 0;
  font-size: 0.75rem;
  color: var(--text-secondary);
}
</style>
