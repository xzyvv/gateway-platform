<template>
  <div>
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="下载模板" icon="Document" />
      </template>
      <el-steps :active="step" finish-status="success" align-center>
        <el-step title="下载模板" />
        <el-step title="上传文件" />
        <el-step title="校验预览" />
        <el-step title="确认导入" />
      </el-steps>

      <div class="step-content">
        <!-- Step 1: 下载模板 -->
        <div v-if="step === 0" class="step-body">
          <EmptyState description="下载设备标准 CSV 模板，按格式填写设备信息（模板列须与网关模板库中的模板一致）" :image-size="80" />
          <el-button type="primary" @click="downloadTpl"><el-icon><Download /></el-icon> 下载 CSV 模板</el-button>
        </div>

        <!-- Step 2: 上传文件 -->
        <div v-else-if="step === 1" class="step-body">
          <ImportFileUpload
            :raw-text="rawText"
            :preview-lines="previewLines"
            @file-change="onFileChange"
          />
        </div>

        <!-- Step 3: 校验预览 -->
        <div v-else-if="step === 2" class="step-body">
          <ImportValidatePreview
            :rows="result.rows"
            :columns="resultColumns"
            :total="result.total"
            :valid="result.valid"
            :invalid="result.invalid"
            :loading="loading"
            :max-height="300"
          />
        </div>

        <!-- Step 4: 确认导入 -->
        <div v-else-if="step === 3" class="step-body">
          <ImportResultPanel
            :icon="importDone ? 'success' : 'info'"
            :title="importDone ? `成功导入 ${importCount} 台设备，失败 ${importFailed} 台` : '确认导入以下有效设备'"
            :sub-title="importDone ? '' : `共 ${result.valid} 台有效设备，跳过 ${result.invalid} 台无效设备`"
          >
            <template v-if="!importDone" #extra>
              <el-button type="primary" :loading="loading" @click="doImport"><el-icon><Check /></el-icon> 确认导入</el-button>
            </template>
          </ImportResultPanel>
          <DataTable
            v-if="importDone && outcomes.length > 0"
            :data="outcomes"
            :columns="outcomeColumns"
            row-key="line"
            stripe
            :max-height="260"
            empty-text="暂无导入结果"
          />
        </div>
      </div>

      <div class="step-actions">
        <el-button v-if="step > 0 && !loading" @click="step--">上一步</el-button>
        <el-button v-if="step === 0" type="primary" @click="step++">下一步</el-button>
        <el-button v-if="step === 1 && rawText" type="primary" @click="parseAndValidate">下一步</el-button>
        <el-button v-if="step === 2" type="primary" @click="step++">下一步</el-button>
        <el-button v-if="step === 3 && importDone" type="primary" @click="reset">开始新的导入</el-button>
      </div>
    </el-card>

    <!-- 历史记录（设备导入会话历史，点位分支不产生该记录） -->
    <el-card shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="导入历史" icon="Clock" />
      </template>
      <DataTable :data="history" :columns="historyColumns" row-key="time" stripe :loading="loading" empty-text="暂无历史记录" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
/**
 * 批量导入 · 设备分支（下载模板 → 上传文件 → 校验预览 → 确认导入）。
 *
 * 沿用原「设备批量导入」页的全部行为：本地解析校验后逐行调用 createDevice，
 * 导入结果（成功 / 失败原因）与本次会话的导入历史都来自真实接口结果。
 *
 * Props:
 *   - templates: 网关模板库（用于本地校验「模板存在」与「模板协议 = 设备协议」，
 *                拉取失败时留空交给后端兜底）
 *
 * 典型应用场景：批量导入页选择「设备」导入对象时展示。
 */
import { ref, computed, h } from 'vue'
import { ElMessage, type UploadFile } from 'element-plus'
import { Download, Check } from '@element-plus/icons-vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { parseCSV, validateImport, downloadTemplate, type ImportResult, type ImportRow, type TemplateInfo } from '@/utils/batchImportCsv'
import { createDevice } from '@/api/devices'
import type { DeviceConfig } from '@/api/types'
import DataTable, { type ColumnDef } from '@/components/DataTable.vue'
import { timeColumn, protocolColumn, addressColumn, tagColumn } from '@/utils/commonColumns'
import SectionHeader from '@/components/SectionHeader.vue'
import EmptyState from '@/components/EmptyState.vue'
import ImportFileUpload from './ImportFileUpload.vue'
import ImportValidatePreview from './ImportValidatePreview.vue'
import ImportResultPanel from './ImportResultPanel.vue'
import type { HistoryRow, OutcomeRow } from './types'

const { showToast } = useErrorDisplay()

const props = defineProps<{
  templates: TemplateInfo[]
}>()

const step = ref(0)
const rawText = ref('')
const fileName = ref('')
const result = ref<ImportResult>({ total: 0, valid: 0, invalid: 0, rows: [] })
const importDone = ref(false)
const importCount = ref(0)
const importFailed = ref(0)
/** 提交中：导入请求往返期间表格显示加载态，避免用户重复点击 */
const loading = ref(false)

/** 本次会话的导入历史（结果来自真实接口，不做持久化） */
const history = ref<HistoryRow[]>([])

/** 逐行导入结果（成功/失败原因） */
const outcomes = ref<OutcomeRow[]>([])

const previewLines = computed(() => rawText.value.split('\n').slice(0, 5).join('\n'))

// 校验预览表：列与原 el-table 对齐，行数有限，超阈值由 DataTable 自动切虚拟滚动
const resultColumns: ColumnDef<ImportRow>[] = [
  { key: 'line', title: '行号', width: 60 },
  { key: 'name', title: '设备名称', width: 120 },
  { key: 'id', title: '设备ID', width: 120 },
  { key: 'bus', title: '所属总线', width: 140 },
  protocolColumn<ImportRow>({ width: 90, useLabel: false }),
  { key: 'slaveId', title: '从站', width: 70 },
  { key: 'template', title: '模板', width: 130, ellipsis: true },
  addressColumn<ImportRow>({ width: 100 }),
  tagColumn<ImportRow>('valid', {
    title: '状态',
    width: 80,
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

// 导入结果表：逐行展示后端返回的成败与原因
const outcomeColumns: ColumnDef<OutcomeRow>[] = [
  { key: 'line', title: '行号', width: 60 },
  { key: 'name', title: '设备名称', width: 140 },
  { key: 'id', title: '设备ID', width: 140 },
  tagColumn<OutcomeRow>('ok', {
    title: '结果',
    width: 80,
    type: (row) => (row.ok ? 'success' : 'danger'),
    label: (row) => (row.ok ? '成功' : '失败'),
  }),
  { key: 'error', title: '失败原因', minWidth: 200, ellipsis: true, cell: (row) => row.error || '无' },
]

// 导入历史表
const historyColumns: ColumnDef<HistoryRow>[] = [
  timeColumn<HistoryRow>('time'),
  { key: 'fileName', title: '文件名', ellipsis: true },
  { key: 'total', title: '导入总数', width: 90 },
  { key: 'success', title: '成功', width: 90 },
  { key: 'failed', title: '失败', width: 90 },
]

function downloadTpl() {
  downloadTemplate()
  ElMessage.success('模板已开始下载')
}

function onFileChange(file: UploadFile) {
  const reader = new FileReader()
  reader.onload = (e) => {
    rawText.value = String(e.target?.result || '')
    fileName.value = file.name
  }
  reader.readAsText(file.raw!, 'utf-8')
}

function parseAndValidate() {
  if (!rawText.value) { ElMessage.warning('请先上传文件'); return }
  const rows = parseCSV(rawText.value)
  result.value = validateImport(rows, props.templates)
  step.value = 2
}

async function doImport() {
  const validRows = result.value.rows.filter((r) => r.valid)
  if (validRows.length === 0) { ElMessage.warning('没有可导入的有效设备'); return }
  loading.value = true
  try {
    const rows: OutcomeRow[] = []
    let success = 0
    // 逐行下发：后端 create_device 每次都会落盘并热重载整份配置，
    // 串行调用可避免并发「读→改→写」互相覆盖（服务端虽有配置锁，串行更直观可控）
    for (const r of validRows) {
      try {
        await createDevice(buildPayload(r))
        rows.push({ line: r.line, name: r.name, id: r.id, ok: true })
        success += 1
      } catch (e) {
        rows.push({ line: r.line, name: r.name, id: r.id, ok: false, error: e instanceof Error ? e.message : String(e) })
      }
    }
    outcomes.value = rows
    importCount.value = success
    importFailed.value = rows.length - success
    importDone.value = true
    history.value.unshift({
      time: Date.now(), fileName: fileName.value,
      total: result.value.total, success, failed: rows.length - success + result.value.invalid,
    })
    if (importFailed.value > 0) {
      ElMessage.warning(`导入完成：成功 ${success} 台，失败 ${importFailed.value} 台，详见下方结果`)
    } else {
      ElMessage.success(`成功导入 ${success} 台设备`)
    }
  } catch (e) {
    showToast(e, '导入失败')
  } finally {
    loading.value = false
  }
}

/** 组装后端 POST /api/devices 的请求体（interval 下发 null 表示沿用模板默认周期） */
function buildPayload(r: ImportRow): DeviceConfig {
  const isModbus = r.protocol === 'modbus-tcp' || r.protocol === 'modbus-rtu'
  return {
    id: r.id,
    name: r.name,
    template: r.template,
    bus: r.bus,
    protocol: r.protocol,
    address: r.address,
    slave_id: isModbus ? Number(r.slaveId) : null,
    address_map: null,
    // interval 为 null 时由模板 default_interval 决定，避免被覆盖成 0
    interval: null,
  } as unknown as DeviceConfig
}

function reset() {
  step.value = 0
  rawText.value = ''
  fileName.value = ''
  result.value = { total: 0, valid: 0, invalid: 0, rows: [] }
  importDone.value = false
  importCount.value = 0
  importFailed.value = 0
  outcomes.value = []
}
</script>

<style scoped>
.section-card { border-radius: var(--radius-card); }
.step-body { padding: 24px 0; text-align: center; min-height: 200px; display: flex; flex-direction: column; align-items: center; justify-content: center; }
.step-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; }
</style>
