<template>
  <div>
    <el-upload
      drag
      :auto-upload="false"
      :on-change="onChange"
      accept=".csv,.txt"
      :show-file-list="false"
    >
      <el-icon class="el-icon--upload"><UploadFilled /></el-icon>
      <div class="el-upload__text">将 CSV 文件拖到此处，或<em>点击上传</em></div>
    </el-upload>
    <div v-if="rawText" class="preview-text">
      <div class="preview-label">文件内容预览（前 5 行校验后）</div>
      <pre>{{ previewLines }}</pre>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 批量导入 · 共用「文件上传」区块。
 *
 * 设备分支与点位分支的上传交互完全一致（拖拽上传 + 前 5 行内容预览），
 * 只有「拿到文件后怎么处理」不同，因此这里只负责渲染与把文件抛给调用方。
 *
 * Props:
 *   - rawText:     已读入的文件文本（为空则不渲染预览）
 *   - previewLines: 预览文本（通常是文件前 5 行）
 *
 * Emits:
 *   - file-change: 选择了文件（payload 为 element-plus 的 UploadFile），
 *                  由调用方决定读取方式与后续校验
 *
 * 典型应用场景：批量导入页的设备分支「上传文件」步与点位分支 CSV「上传文件」步。
 */
import { UploadFilled } from '@element-plus/icons-vue'
import type { UploadFile } from 'element-plus'

defineProps<{
  rawText: string
  previewLines: string
}>()

const emit = defineEmits<{
  'file-change': [file: UploadFile]
}>()

/** 文件选择变化：原样上抛，读取与校验交给调用方 */
function onChange(file: UploadFile) {
  emit('file-change', file)
}
</script>

<style scoped>
.preview-text { margin-top: 16px; text-align: left; width: 100%; max-width: 600px; }
.preview-label { font-size: 0.75rem; color: var(--text-secondary); margin-bottom: 6px; }
.preview-text pre { background: var(--bg-fill); padding: 12px; border-radius: var(--radius-button); font-size: 0.75rem; overflow-x: auto; }
</style>
