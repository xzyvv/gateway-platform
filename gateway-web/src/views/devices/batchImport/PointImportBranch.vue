<template>
  <el-card shadow="never" class="section-card">
    <template #header>
      <div class="card-header">
        <SectionHeader title="点位导入" icon="Document" />
        <el-radio-group v-model="pointMode" size="small" aria-label="导入方式">
          <el-radio-button value="csv">CSV 文件</el-radio-button>
          <el-radio-button value="json">粘贴 JSON</el-radio-button>
        </el-radio-group>
      </div>
    </template>

    <el-steps :active="pointStep" finish-status="success" align-center>
      <el-step
        v-for="(title, idx) in pointStepTitles"
        :key="title"
        :title="title"
        :class="{ 'is-clickable': idx <= pointMaxStep && idx !== pointStep }"
        @click="goPointStep(idx)"
      />
    </el-steps>

    <div class="step-content">
      <!-- CSV Step 1: 下载模板 -->
      <div v-if="pointMode === 'csv' && pointStep === 0" class="step-body">
        <PointCsvTemplateStep @download-template="downloadPointTpl" />
      </div>

      <!-- CSV Step 2: 上传文件 -->
      <div v-else-if="pointMode === 'csv' && pointStep === 1" class="step-body">
        <ImportFileUpload
          :raw-text="pointRawText"
          :preview-lines="pointPreviewLines"
          @file-change="onPointFileChange"
        />
      </div>

      <!-- JSON Step 1: 粘贴 JSON -->
      <div v-else-if="pointMode === 'json' && pointStep === 0" class="step-body json-body">
        <PointJsonInputStep v-model:text="pointJsonText" />
      </div>

      <!-- 校验预览（两种方式共用） -->
      <div v-else-if="pointShowPreview" class="step-body">
        <ImportValidatePreview
          :rows="pointResult.rows"
          :columns="pointRowColumns"
          :total="pointResult.total"
          :valid="pointResult.valid"
          :invalid="pointResult.invalid"
          :loading="false"
          :max-height="320"
        />
      </div>

      <!-- 确认导入 -->
      <div v-else-if="pointConfirmStep" class="step-body">
        <ImportResultPanel
          :icon="pointImportDone ? 'success' : 'info'"
          :title="pointImportDone ? `成功导入 ${pointImportedCount} 个点位` : '确认导入以下有效数据'"
          :sub-title="pointImportDone ? '' : `共 ${pointResult.valid} 行有效数据，跳过 ${pointResult.invalid} 行无效数据`"
        >
          <template #extra>
            <el-button v-if="!pointImportDone" type="primary" :loading="pointImporting" @click="doImportPoints">
              <el-icon><Check /></el-icon> 确认导入
            </el-button>
            <template v-else>
              <el-button type="primary" @click="resetPoint">开始新的导入</el-button>
              <el-button @click="router.push('/points')">前往点位列表</el-button>
            </template>
          </template>
        </ImportResultPanel>
      </div>
    </div>

    <div class="step-actions">
      <el-button v-if="pointStep > 0" @click="pointStep--">上一步</el-button>
      <!-- CSV 第 1 步：必须给出前进路径，否则流程会卡死在模板下载 -->
      <el-button v-if="pointMode === 'csv' && pointStep === 0" type="primary" @click="advancePointStep(1)">下一步：上传文件</el-button>
      <el-button v-if="pointMode === 'csv' && pointStep === 1 && pointRawText" type="primary" @click="parseAndValidatePointCsv">下一步：校验</el-button>
      <el-button v-if="pointMode === 'json' && pointStep === 0" type="primary" :disabled="!pointJsonText.trim()" @click="parsePointJson">校验数据</el-button>
      <el-button v-if="pointShowPreview" type="primary" @click="advancePointStep(pointPreviewStep + 1)">下一步：确认导入</el-button>
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 批量导入 · 点位分支（CSV 文件 / 粘贴 JSON 两种来源）。
 *
 * 原「点位 CSV 批量导入」页已删除，其 CSV 模板下载、上传解析校验预览、确认导入
 * 以及 JSON 粘贴导入能力全部并入这里：两种来源共用 el-steps 骨架与校验预览步，
 * 状态各自独立（pointStep / pointResult / pointPayload），切换来源互不污染。
 *
 * 服务端契约不变：点位调用 batchImportPoints。
 *
 * 典型应用场景：批量导入页选择「点位」导入对象时展示。
 */
import { ref, computed, h, watch } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, type UploadFile } from 'element-plus'
import { Check } from '@element-plus/icons-vue'
import { downloadCsvText } from '@/utils/exportCsv'
import { parseImportPoints } from '@/utils/importPoints'
import { batchImportPoints } from '@/api/points'
import type { PointConfig } from '@/api/types'
import type { ColumnDef } from '@/components/DataTable.vue'
import { protocolColumn, addressColumn, tagColumn } from '@/utils/commonColumns'
import SectionHeader from '@/components/SectionHeader.vue'
import ImportFileUpload from './ImportFileUpload.vue'
import ImportValidatePreview from './ImportValidatePreview.vue'
import ImportResultPanel from './ImportResultPanel.vue'
import PointCsvTemplateStep from './PointCsvTemplateStep.vue'
import PointJsonInputStep from './PointJsonInputStep.vue'
import { parsePointCsvRows } from './pointCsv'
import type { PointParseResult, PointRow } from './types'

const router = useRouter()

const pointMode = ref<'csv' | 'json'>('csv')
const pointStep = ref(0)
/** 已解锁的最远步骤：步骤条只允许回跳到已解锁的位置 */
const pointMaxStep = ref(0)
const pointRawText = ref('')
const pointJsonText = ref('')
const pointResult = ref<PointParseResult>({ total: 0, valid: 0, invalid: 0, rows: [] })
/** 真正提交给后端的数据（CSV 行与 JSON 行各自由自己的规则归一，避免默认值覆盖用户输入） */
const pointPayload = ref<PointConfig[]>([])
const pointImporting = ref(false)
const pointImportDone = ref(false)
const pointImportedCount = ref(0)

const pointStepTitles = computed(() =>
  pointMode.value === 'csv'
    ? ['下载模板', '上传文件', '校验预览', '确认导入']
    : ['粘贴 JSON', '校验预览', '确认导入'],
)
/** 预览步骤：CSV 为第 3 步（索引 2），JSON 为第 2 步（索引 1） */
const pointPreviewStep = computed(() => (pointMode.value === 'csv' ? 2 : 1))
const pointShowPreview = computed(() => pointStep.value === pointPreviewStep.value)
const pointConfirmStep = computed(() => pointStep.value === pointPreviewStep.value + 1)

const pointPreviewLines = computed(() => pointRawText.value.split('\n').slice(0, 5).join('\n'))

/** 步骤条回跳：只允许跳回已解锁步骤 */
function goPointStep(idx: number) {
  if (idx > pointMaxStep.value) return
  pointStep.value = idx
}

/**
 * 前进到指定步骤：前进不受回跳守卫限制，并同步解锁该步骤。
 * 之前「下一步」按钮复用 goStep，而 maxStep 初始为 0，导致 goStep(1) 被直接拦截，
 * 页面永远停在第 1 步、文件控件不渲染，CSV 导入整条链路不可用。
 */
function advancePointStep(idx: number) {
  pointMaxStep.value = Math.max(pointMaxStep.value, idx)
  pointStep.value = idx
}

watch(pointMode, () => {
  resetPoint()
})

const pointRowColumns: ColumnDef<PointRow>[] = [
  { key: 'line', title: '行号', width: 60 },
  { key: 'id', title: '点位 ID', minWidth: 120, ellipsis: true },
  { key: 'name', title: '点位名称', minWidth: 120, ellipsis: true },
  { key: 'bus', title: '所属总线', width: 100, ellipsis: true },
  protocolColumn<PointRow>({ width: 80, useLabel: false }),
  addressColumn<PointRow>({ minWidth: 100 }),
  { key: 'slave_id', title: '从站', width: 70 },
  tagColumn<PointRow>('valid', {
    title: '状态',
    width: 70,
    type: (row) => (row.valid ? 'success' : 'danger'),
    label: (row) => (row.valid ? '有效' : '无效'),
  }),
  {
    key: 'error',
    title: '校验错误信息',
    minWidth: 160,
    ellipsis: true,
    cell: (row) => h('span', { class: 'error-text' }, row.error || '无'),
  },
]

/** 生成点位 CSV 模板 */
function downloadPointTpl() {
  const header = 'id,name,device_id,bus,protocol,address,slave_id,address_num,ct_ratio,pt_ratio\n'
  const example = 'temp_001,温度传感器1,dev_001,bus1,modbus,40001,1,1,,\n'
  const example2 = 'voltage_001,电压采样1,dev_002,bus2,dlt645,09000001,,1,,100\n'
  // 走统一入口：BOM 由 downloadCsvText 附加，否则 Excel 打开示例里的中文是乱码
  downloadCsvText(header + example + example2, 'points_template.csv')
  ElMessage.success('模板已开始下载，填好后点「下一步：上传文件」')
}

function onPointFileChange(file: UploadFile) {
  if (!file.raw) {
    ElMessage.warning('文件无效')
    return
  }
  const reader = new FileReader()
  reader.onload = (e) => {
    pointRawText.value = String(e.target?.result || '')
  }
  reader.readAsText(file.raw, 'utf-8')
}

/**
 * 点位 CSV 解析与校验。
 *
 * 解析与默认值补齐放在 ./pointCsv.ts（纯函数），这里只负责把结果写回本组件的状态并推进步骤。
 */
function parseAndValidatePointCsv() {
  const outcome = parsePointCsvRows(pointRawText.value)
  if (!outcome.ok) { ElMessage.warning(outcome.message); return }
  pointResult.value = outcome.result
  pointPayload.value = outcome.payload
  pointMaxStep.value = 3
  pointStep.value = 2
}

/** JSON 粘贴：逐字段类型校验后进入预览（复用 @/utils/importPoints 的统一口径） */
function parsePointJson() {
  const { valid, error } = parseImportPoints(pointJsonText.value)
  if (valid.length === 0) {
    ElMessage.warning(error || '未找到有效点位')
    return
  }
  if (error) ElMessage.warning(error)
  const rows: PointRow[] = valid.map((p, i) => ({ ...p, line: i + 1, valid: true }))
  pointResult.value = { total: valid.length, valid: valid.length, invalid: 0, rows }
  pointPayload.value = valid
  pointMaxStep.value = 2
  pointStep.value = 1
}

async function doImportPoints() {
  if (pointPayload.value.length === 0) { ElMessage.warning('没有有效数据可导入'); return }
  pointImporting.value = true
  try {
    await batchImportPoints(pointPayload.value)
    pointImportedCount.value = pointPayload.value.length
    pointImportDone.value = true
    ElMessage.success(`成功导入 ${pointPayload.value.length} 个点位`)
  } catch {
    ElMessage.error('导入失败，请检查数据后重试')
  } finally {
    pointImporting.value = false
  }
}

function resetPoint() {
  pointStep.value = 0
  pointMaxStep.value = 0
  pointRawText.value = ''
  pointJsonText.value = ''
  pointPayload.value = []
  pointResult.value = { total: 0, valid: 0, invalid: 0, rows: [] }
  pointImportDone.value = false
  pointImportedCount.value = 0
}
</script>

<style scoped>
.section-card { border-radius: var(--radius-card); }
.card-header { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
.step-body { padding: 24px 0; text-align: center; min-height: 200px; display: flex; flex-direction: column; align-items: center; justify-content: center; }
.json-body { align-items: stretch; }
.step-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; }
.is-clickable { cursor: pointer; }
</style>
