<template>
  <el-dialog v-model="visible" title="配置校验报告" width="720px">
    <div class="val-summary">
      <el-tag type="danger" size="large">错误 {{ validateErrors }}</el-tag>
      <el-tag type="warning" size="large">警告 {{ validateWarnings }}</el-tag>
      <el-tag :type="validateErrors === 0 ? 'success' : 'info'" size="large">
        {{ validateErrors === 0 ? '校验通过' : '需修复' }}
      </el-tag>
    </div>
    <DataTable
      :data="validateIssues"
      :columns="validateColumns"
      :row-key="validateRowKey"
      :max-height="400"
      :empty-text="validating ? '正在校验…' : '未发现问题'"
    />
    <template #footer>
      <el-button @click="visible = false">关闭</el-button>
      <el-button type="primary" @click="visible = false; emit('save')" :disabled="validating || validateErrors > 0">
        校验通过，去保存
      </el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch, h } from 'vue'
import { ElButton, ElMessage, ElTag } from 'element-plus'
import { validateConfig, type ConfigIssue } from '@/utils/configValidation'
import type { GatewayConfig } from '@/api/types'
import { useDialogVisible } from '@/composables/core/useDialogVisible'
import type { ColumnDef } from '@/components/DataTable.vue'

const props = defineProps<{
  modelValue: boolean
  config: GatewayConfig
  parseError: boolean
}>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  locate: [tab: string]
  save: []
}>()

const visible = useDialogVisible(props, emit)

const validateIssues = ref<ConfigIssue[]>([])
/** 后端校验请求进行中（规则在后端，需等待往返） */
const validating = ref(false)
const validateErrors = computed(() => validateIssues.value.filter((i) => i.level === 'error').length)
const validateWarnings = computed(() => validateIssues.value.filter((i) => i.level === 'warning').length)

/** 校验问题行唯一键（ConfigIssue 无 id 字段，用级别+字段+描述组合） */
function validateRowKey(row: ConfigIssue): string {
  return `${row.level}:${row.field}:${row.message}`
}

// 校验报告列定义，顺序/宽度与原 el-table 保持一致
const validateColumns: ColumnDef<ConfigIssue>[] = [
  {
    key: 'level',
    title: '级别',
    width: 80,
    cell: (row) =>
      h(
        ElTag,
        {
          type: row.level === 'error' ? 'danger' : row.level === 'warning' ? 'warning' : 'info',
          size: 'small',
        },
        () => (row.level === 'error' ? '错误' : row.level === 'warning' ? '警告' : '提示'),
      ),
  },
  { key: 'message', title: '问题描述', minWidth: 280 },
  { key: 'field', title: '字段', width: 200, ellipsis: true },
  {
    key: 'action',
    title: '定位',
    width: 80,
    cell: (row) =>
      row.tab
        ? h(ElButton, { size: 'small', link: true, type: 'primary', onClick: () => locateIssue(row) }, () => '定位')
        : null,
  },
]

async function runValidation() {
  if (props.parseError) {
    ElMessage.warning('JSON 格式错误，请修正后再校验')
    visible.value = false
    return
  }
  validating.value = true
  try {
    validateIssues.value = await validateConfig(props.config)
  } catch (e) {
    validateIssues.value = []
    ElMessage.error(`配置校验请求失败：${(e as Error)?.message ?? '未知错误'}`)
    visible.value = false
  } finally {
    validating.value = false
  }
}

function locateIssue(issue: ConfigIssue) {
  if (issue.tab) {
    emit('locate', issue.tab)
  }
  visible.value = false
}

watch(visible, (v) => { if (v) void runValidation() })
</script>

<style scoped>
.val-summary { display: flex; gap: 8px; margin-bottom: 12px; }
</style>
