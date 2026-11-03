<template>
  <!--
    键盘快捷键帮助：集中列出当前实际生效的键位（不写未实现的），
    避免文档与实现再次脱节。
  -->
  <el-dialog v-model="visible" title="键盘快捷键" width="560px" append-to-body>
    <ul class="sc-list">
      <li v-for="s in shortcuts" :key="s.label" class="sc-item">
        <span class="sc-keys">
          <kbd v-for="k in s.keys" :key="k">{{ k }}</kbd>
        </span>
        <span class="sc-label">{{ s.label }}</span>
      </li>
    </ul>
    <template #footer>
      <el-button type="primary" @click="visible = false">知道了</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{ modelValue: boolean }>()
const emit = defineEmits<{ 'update:modelValue': [value: boolean] }>()

const visible = computed({
  get: () => props.modelValue,
  set: (v) => emit('update:modelValue', v),
})

/**
 * 仅收录代码中真实存在的键位：
 * - Ctrl+Shift+M：Layout.onGlobalKeydown
 * - ?：Layout.onGlobalKeydown
 * - Enter / F2 / Esc：InlineEditCell（进入编辑、保存、取消）
 * - Esc 关闭浮层：Element Plus 默认行为
 */
const shortcuts = [
  { keys: ['Ctrl', 'Shift', 'M'], label: '开关请求监视器（调试接口 / WS 帧）' },
  { keys: ['?'], label: '打开本快捷键说明' },
  { keys: ['Enter'], label: '表格单元格：进入行内编辑；编辑中：保存' },
  { keys: ['F2'], label: '表格单元格：进入行内编辑' },
  { keys: ['Esc'], label: '编辑中：放弃修改；弹窗/浮层：关闭' },
  { keys: ['Tab', 'Shift+Tab'], label: '在可聚焦控件之间前后移动' },
]
</script>

<style scoped>
.sc-list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.sc-item {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 9px 0;
  border-bottom: 1px dashed var(--border-lighter);
  font-size: 0.8125rem;
}
.sc-item:last-child { border-bottom: none; }
.sc-keys {
  display: flex;
  gap: 4px;
  flex: 0 0 150px;
}
kbd {
  display: inline-block;
  min-width: 22px;
  padding: 2px 6px;
  border: 1px solid var(--border-base);
  border-bottom-width: 2px;
  border-radius: var(--radius-small);
  background: var(--bg-fill);
  color: var(--text-primary);
  font-family: inherit;
  font-size: 0.75rem;
  line-height: 18px;
  text-align: center;
}
.sc-label { color: var(--text-regular); line-height: 1.5; }
</style>
