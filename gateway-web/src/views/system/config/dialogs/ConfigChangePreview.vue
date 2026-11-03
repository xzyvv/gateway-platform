<template>
  <el-dialog
    v-model="visible"
    title="配置变更预览"
    width="720px"
    class="diff-dialog"
  >
    <div class="diff-stats" v-if="hasLoaded">
      <el-tag v-if="hasChanges" type="warning" effect="dark" round size="small">
        {{ changedFields.length }} 处字段变更
      </el-tag>
      <el-tag v-else type="info" effect="plain" round size="small">无变更</el-tag>
    </div>
    <!-- diff 区：v-loading 换成分支渲染骨架条（左黄右绿暗示 add/remove 语义） -->
    <div v-if="diffLoading" class="diff-body diff-skeleton">
      <div class="diff-sk-line sk-add">
        <el-skeleton animated><template #template><el-skeleton-item variant="text" /></template></el-skeleton>
      </div>
      <div class="diff-sk-line sk-remove">
        <el-skeleton animated><template #template><el-skeleton-item variant="text" /></template></el-skeleton>
      </div>
      <div class="diff-sk-line sk-context">
        <el-skeleton animated><template #template><el-skeleton-item variant="text" /></template></el-skeleton>
      </div>
      <div class="diff-sk-line sk-add">
        <el-skeleton animated><template #template><el-skeleton-item variant="text" /></template></el-skeleton>
      </div>
      <div class="diff-sk-line sk-context">
        <el-skeleton animated><template #template><el-skeleton-item variant="text" /></template></el-skeleton>
      </div>
    </div>
    <div v-else class="diff-body">
      <div v-if="hasChanges" class="changed-fields">
        <el-tag v-for="f in changedFields" :key="f" size="small" type="warning" effect="plain" round>
          {{ fieldLabel(f) }}
        </el-tag>
      </div>
      <empty-state v-else description="无变更内容" :image-size="60" />
    </div>
    <template #footer>
      <el-button @click="visible = false">关闭</el-button>
      <el-button type="primary" @click="visible = false; emit('save')">
        <el-icon><Check /></el-icon> {{ confirmText || '确认保存' }}
      </el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import { Check } from '@element-plus/icons-vue'
import { ElMessage } from 'element-plus'
import { configDiff } from '@/api/config'
import type { GatewayConfig } from '@/api/types'
import { fieldLabel } from '@/composables/config/useConfigVersions'
import { useDialogVisible } from '@/composables/core/useDialogVisible'

const props = defineProps<{
  modelValue: boolean
  config: GatewayConfig
  parseError: boolean
  /** 主按钮文案：保存流程用默认「确认保存」，导入流程传「确认导入」 */
  confirmText?: string
}>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  save: []
}>()

const visible = useDialogVisible(props, emit)

const diffLoading = ref(false)

const changedFields = ref<string[]>([])
const hasChanges = ref(false)
/** 首次请求返回后置 true，用于显示顶部统计条（避免加载中显示「无变更」） */
const hasLoaded = ref(false)

/** 后端 config_diff 契约：{ changed_fields: string[]; has_changes: boolean } */
interface ChangedFieldsResult {
  changed_fields: string[]
  has_changes?: boolean
}
function isChangedFieldsResult(raw: unknown): raw is ChangedFieldsResult {
  return !!raw && typeof raw === 'object' && Array.isArray((raw as ChangedFieldsResult).changed_fields)
}

async function previewDiff() {
  if (props.parseError) {
    ElMessage.warning('JSON 格式错误，请先修正')
    visible.value = false
    return
  }
  diffLoading.value = true
  changedFields.value = []
  hasChanges.value = false
  hasLoaded.value = false
  try {
    const raw: unknown = await configDiff(props.config)
    // 后端契约固定为 { changed_fields, has_changes }，非此形状视为异常（不再兼容旧 diff 格式）
    if (isChangedFieldsResult(raw)) {
      changedFields.value = raw.changed_fields
      // 取「后端标记」与「字段列表非空」的并集，避免任一字段不可信时退回"无变更"
      hasChanges.value = raw.has_changes === true || raw.changed_fields.length > 0
    }
  } catch {
    // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
    ElMessage.error('配置变更预览失败，请稍后重试')
  } finally {
    diffLoading.value = false
    hasLoaded.value = true
  }
}

// immediate：调用方若用 v-if 挂载（打开时才创建组件），首次渲染 visible 已为 true，
// 非 immediate 的 watch 不会触发，diff 区会一直停在「无变更内容」。
watch(visible, (v) => { if (v) previewDiff() }, { immediate: true })
</script>

<style scoped>
.diff-stats { display: flex; gap: 8px; margin-bottom: 12px; flex-wrap: wrap; }
.diff-body { min-height: 200px; }
.changed-fields { display: flex; flex-wrap: wrap; gap: 8px; }

/* diff 加载骨架：左黄右绿暗示 add/remove 语义 */
.diff-skeleton {
  background: var(--bg-card-header);
  padding: 12px;
  border-radius: var(--radius-small);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.diff-sk-line { padding: 4px 0; }
.diff-sk-line.sk-add { background: rgba(var(--color-success-rgb), 0.12); }
.diff-sk-line.sk-remove { background: rgba(var(--color-danger-rgb), 0.12); }
</style>
