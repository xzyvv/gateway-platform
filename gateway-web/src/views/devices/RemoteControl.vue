<template>
  <div class="rc">
    <SectionHeader title="远程控制" icon="Operation" />

    <el-row :gutter="16">
      <el-col :span="8">
        <el-card shadow="never" class="section-card">
          <template #header><strong>下行写命令</strong></template>
          <el-form label-width="80px" size="default" ref="formRef">
            <el-form-item label="点位">
              <el-select v-model="selPoint" filterable style="width:100%" placeholder="选择可写点位" :loading="loadingPoints">
                <el-option v-for="p in writablePoints" :key="p.id" :label="`${p.device_name} - ${p.name}`" :value="p.id" />
              </el-select>
            </el-form-item>
            <el-form-item label="写入值" prop="writeValue">
              <el-input v-model="writeValue" :placeholder="selPoint ? `类型: ${pointType}` : '请先选择点位'" />
            </el-form-item>
            <el-form-item label="操作人"><el-input v-model="operator" placeholder="请输入操作人" /></el-form-item>
            <el-form-item>
              <el-button type="primary" @click="send" :disabled="!selPoint || !writeValue || sending">
                <el-icon><Promotion /></el-icon> {{ sending ? '下发中...' : '下发命令' }}
              </el-button>
            </el-form-item>
          </el-form>
          <el-alert v-if="lastResult" :title="lastResult" :type="lastType" :closable="false" show-icon />
        </el-card>
      </el-col>
      <el-col :span="16">
        <el-card shadow="never" class="section-card">
          <template #header><strong>下行执行历史</strong> <el-button size="small" link @click="load">刷新</el-button></template>
          <DataTable
            :data="commands"
            :columns="commandColumns"
            row-key="id"
            stripe
          >
            <template #empty>
              <empty-state description="暂无远程控制记录，请先在点位管理中确认可写点位后下发" :image-size="80">
                <el-button type="primary" size="small" @click="$router.push('/points')">去点位管理</el-button>
              </empty-state>
            </template>
          </DataTable>
        </el-card>
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, h } from 'vue'
import { ElMessage, ElTag, type FormInstance } from 'element-plus'
import { Promotion } from '@element-plus/icons-vue'
import { listWritablePoints, listCommands, type ControlCommand, type WritablePoint } from '@/api/remoteControl'
import { confirmPointWrite, usePointWrite } from '@/composables/data/usePointWrite'
import { setFormFieldErrors, clearFormFieldErrors } from '@/utils/formFieldError'
import type { ColumnDef } from '@/components/DataTable.vue'
import { formatTimeOfDay } from '@/utils/datetime'

const STATUS_LABEL: Record<string, string> = { success: '成功', failed: '失败', pending: '待执行' }
const STATUS_COLOR: Record<string, string> = { success: 'success', failed: 'danger', pending: 'warning' }

function statusColor(s: string): string { return STATUS_COLOR[s] ?? 'info' }
function statusLabel(s: string): string { return STATUS_LABEL[s] ?? s }

// DataTable 列定义：宽度/顺序与原列保持一致
const commandColumns: ColumnDef<ControlCommand>[] = [
  { key: 'device_name', title: '设备', width: 110 },
  { key: 'point_name', title: '点位', width: 100 },
  { key: 'write_value', title: '写入值', width: 80 },
  { key: 'operator', title: '操作人', width: 80 },
  {
    key: 'status',
    title: '状态',
    width: 90,
    cell: (row) =>
      h(
        ElTag,
        { size: 'small', type: statusColor(row.status) as 'primary' | 'success' | 'info' | 'warning' | 'danger' },
        () => statusLabel(row.status),
      ),
  },
  {
    key: 'created_at',
    title: '下发时间',
    width: 160,
    cell: (row) => h('span', formatTimeOfDay(row.created_at)),
  },
  { key: 'result', title: '结果', minWidth: 180, ellipsis: true },
]

const commands = ref<ControlCommand[]>([])
const writablePoints = ref<WritablePoint[]>([])
const selPoint = ref('')
const writeValue = ref('')
const operator = ref('admin')
const lastResult = ref('')
const lastType = ref<'success' | 'error' | 'info'>('info')
const sending = ref(false)
const loadingPoints = ref(false)
/** el-form 实例引用，供写入值校验高亮 */
const formRef = ref<FormInstance>()

const pointType = computed(() => writablePoints.value.find((p) => p.id === selPoint.value)?.data_type || '')
const { writeOnce } = usePointWrite()

async function loadPoints() {
  loadingPoints.value = true
  try { writablePoints.value = await listWritablePoints() }
  catch { /* ignore */ }
  finally { loadingPoints.value = false }
}

async function load() { commands.value = await listCommands() }

async function send() {
  const point = writablePoints.value.find((p) => p.id === selPoint.value)
  if (!point) { ElMessage.warning('点位不存在'); return }

  const val = Number(writeValue.value)
  // ROI: 数字校验同时高亮写入值字段
  if (isNaN(val)) {
    setFormFieldErrors(formRef.value!, { writeValue: '必须是数字' })
    ElMessage.warning('写入值必须是数字')
    return
  }
  clearFormFieldErrors(formRef.value!, ['writeValue'])
  if (!(await confirmPointWrite([{ id: point.id, value: val }], point.name))) return

  sending.value = true
  lastResult.value = ''
  try {
    // 统一写入口（B2/E2）：单点写也传单元素数组，单/批分发由后端决定
    const outcome = await writeOnce(point.id, val)
    if (outcome.ok) {
      ElMessage.success('下发成功')
      lastResult.value = `写入成功：${outcome.value ?? val}`
      lastType.value = 'success'
    } else {
      ElMessage.error('下发失败')
      lastResult.value = outcome.error
      lastType.value = 'error'
    }
    load()
  } finally {
    sending.value = false
  }
}

onMounted(() => { loadPoints(); load() })
</script>

<style scoped>
.rc { padding: 16px; }
.section-card { margin-bottom: 16px; }
</style>