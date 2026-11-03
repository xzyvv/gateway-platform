<template>
  <div class="raw-tip">
    <el-icon><Warning /></el-icon>
    <span>直接编辑完整 JSON 配置，保存时将整体覆盖。请确保格式正确。</span>
  </div>
  <el-alert
    v-if="parseError"
    type="error"
    :closable="false"
    show-icon
    style="margin-bottom: 10px"
  >
    <template #title>
      <span>JSON 格式错误，请检查语法</span>
      <el-button size="small" type="primary" plain @click="emit('reset')" style="margin-left: 12px">
        <el-icon><RefreshLeft /></el-icon> 恢复为当前配置
      </el-button>
    </template>
    <template v-if="parseErrorMessage" #default>
      <div class="parse-error-detail">{{ parseErrorMessage }}</div>
    </template>
  </el-alert>
  <div class="raw-json-editor">
    <pre class="raw-json-highlight" v-html="rawJsonHtml"></pre>
    <textarea
      v-model="rawText"
      class="raw-json-textarea"
      spellcheck="false"
      placeholder="完整配置 JSON"
      @input="emit('input')"
      @scroll="emit('scroll', $event)"
    ></textarea>
  </div>
</template>

<script setup lang="ts">
// rawText 双向绑定；解析/高亮状态由父组件传入，交互经事件上抛（保持逻辑仍在 useConfigActions）
const rawText = defineModel<string>({ required: true })
defineProps<{ parseError: boolean; parseErrorMessage: string; rawJsonHtml: string }>()
const emit = defineEmits<{
  reset: []
  input: []
  scroll: [e: Event]
}>()
</script>