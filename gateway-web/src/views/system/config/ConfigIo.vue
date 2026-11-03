<template>
  <div class="io">
    <SectionHeader title="导入导出" icon="Document" />

    <el-row :gutter="16">
      <el-col :span="12">
        <el-card shadow="never" class="section-card">
          <template #header><strong>导出配置</strong></template>
          <el-alert
            type="info"
            :closable="false"
            show-icon
            style="margin-bottom: 12px"
            title="导出包含完整网关配置（含 MQTT 凭据等敏感字段），请注意妥善保管"
          />
          <el-button type="primary" @click="doExport" :loading="exporting" :icon="Download">导出 JSON</el-button>
          <el-button @click="copyJson" :icon="Files" :disabled="!exportedJson">复制到剪贴板</el-button>
          <el-button @click="downloadJson" :icon="Download" :disabled="!exportedJson">下载文件</el-button>
          <el-input v-model="exportedJson" type="textarea" :rows="14" readonly aria-label="导出的配置 JSON" class="json-box" />
        </el-card>
      </el-col>
      <el-col :span="12">
        <el-card shadow="never" class="section-card">
          <template #header><strong>导入配置</strong></template>
          <el-alert
            type="warning"
            :closable="false"
            show-icon
            style="margin-bottom: 12px"
            title="导入将覆盖当前网关全部配置并触发热重载，请确认后操作"
          />
          <el-upload :auto-upload="false" :show-file-list="false" accept=".json" :on-change="onFile">
            <el-button :icon="Upload">选择 JSON 文件</el-button>
          </el-upload>
          <el-form ref="importFormRef" :model="importForm" :rules="importRules" scroll-to-error>
            <el-form-item prop="json">
              <el-input v-model="importForm.json" type="textarea" :rows="10" placeholder="粘贴 JSON 内容，或选择文件上传" aria-label="待导入的配置 JSON" class="json-box" />
            </el-form-item>
          </el-form>
          <div class="actions">
            <el-button type="primary" @click="doImport" :disabled="importing" :loading="importing" :icon="Promotion">校验并导入</el-button>
          </div>
          <ImportOutcome
            :visible="!!importResult"
            :success="!!importResult?.success"
            :error="importResult?.error"
            success-title="导入成功，配置已热重载"
            style="margin-top: 12px"
          />
        </el-card>
      </el-col>
    </el-row>

    <el-row :gutter="16">
      <el-col :span="24">
        <el-card shadow="never" class="section-card">
          <template #header><strong>工程文件（总线 / 上行通道 / 变量映射）</strong></template>
          <el-alert
            type="info"
            :closable="false"
            show-icon
            style="margin-bottom: 12px"
            title="工程文件仅含三层可移植子集，不携带本机 Web 鉴权/凭据、NTP 等安全与环境配置；导入只覆盖这三层"
          />
          <div class="actions">
            <el-button type="primary" @click="doExportProject" :loading="exportingProject" :icon="Download">导出工程文件</el-button>
            <el-button @click="copyProjectJson" :icon="Files" :disabled="!projectJson">复制到剪贴板</el-button>
            <el-button @click="downloadProjectJson" :icon="Download" :disabled="!projectJson">下载文件</el-button>
          </div>
          <el-input v-model="projectJson" type="textarea" :rows="12" readonly aria-label="导出的工程文件 JSON" class="json-box" />

          <el-divider />
          <el-upload :auto-upload="false" :show-file-list="false" accept=".json" :on-change="onProjectFile">
            <el-button :icon="Upload">选择工程文件</el-button>
          </el-upload>
          <el-form ref="projectImportFormRef" :model="projectImportForm" :rules="projectImportRules" scroll-to-error>
            <el-form-item prop="json">
              <el-input v-model="projectImportForm.json" type="textarea" :rows="10" placeholder="粘贴工程文件 JSON，或选择文件上传" aria-label="待导入的工程文件 JSON" class="json-box" />
            </el-form-item>
          </el-form>
          <div class="actions">
            <el-button type="primary" @click="doImportProject" :disabled="importingProject" :loading="importingProject" :icon="Promotion">校验并导入</el-button>
          </div>
          <ImportOutcome
            :visible="!!projectImportResult"
            :success="!!projectImportResult?.success"
            :error="projectImportResult?.error"
            success-title="工程文件导入成功，配置已热重载"
            style="margin-top: 12px"
          />
        </el-card>
      </el-col>
    </el-row>

    <!-- 导入前先看影响面：复用保存流程的 configDiff 预览，避免「粘进去就直接覆盖」 -->
    <ConfigChangePreview
      v-if="pendingImport"
      v-model="showImportPreview"
      :config="pendingImport"
      :parse-error="false"
      confirm-text="确认导入"
      @save="confirmImport"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, watch } from 'vue'
import { ElMessage, type FormInstance, type FormRules, type UploadFile } from 'element-plus'
import { Download, Files, Upload, Promotion } from '@element-plus/icons-vue'
import { exportConfig, importConfig } from '@/api/config'
import { exportProject, importProject } from '@/api/project'
import type { GatewayConfig } from '@/api/types'
import type { ProjectFile } from '@/api/project'
import { extractErrorMessage } from '@/utils/errorHandler'
import { triggerDownload } from '@/utils/exportCsv'
import { makeJsonTextValidator } from '@/utils/jsonTextValidator'
import ImportOutcome from '@/components/ImportOutcome.vue'
import ConfigChangePreview from './dialogs/ConfigChangePreview.vue'

const exporting = ref(false)
const importing = ref(false)
const exportedJson = ref('')
// 导入表单模型：JSON 文本放这里，才能做字段级校验（原来只有一条汇总 alert）
const importForm = reactive({ json: '' })
const importFormRef = ref<FormInstance>()
const importResult = ref<{ success: boolean; error?: string } | null>(null)
// 导入两步化：解析通过 → 预览 diff → 确认后才提交
const pendingImport = ref<GatewayConfig | null>(null)
const showImportPreview = ref(false)

// 工程文件（三层子集）状态
const exportingProject = ref(false)
const importingProject = ref(false)
const projectJson = ref('')
const projectImportForm = reactive({ json: '' })
const projectImportFormRef = ref<FormInstance>()
const projectImportResult = ref<{ success: boolean; error?: string } | null>(null)

// JSON 文本校验统一走 utils/jsonTextValidator（与片段导入同一套解析与中文提示），
// 校验失败时原因显示在文本框下方，现场人员一眼看到是哪段 JSON 有问题
const importRules: FormRules = {
  json: [{ validator: makeJsonTextValidator('待导入的配置 JSON'), trigger: 'blur' }],
}
const projectImportRules: FormRules = {
  json: [{ validator: makeJsonTextValidator('待导入的工程文件 JSON'), trigger: 'blur' }],
}

async function doExport() {
  exporting.value = true
  try {
    const cfg = await exportConfig()
    exportedJson.value = JSON.stringify(cfg, null, 2)
    ElMessage.success('配置已导出（凭据字段已脱敏，迁移到新网关需重新填写）')
  } catch (e) {
    ElMessage.error('导出失败：' + (extractErrorMessage(e)))
  } finally {
    exporting.value = false
  }
}

function copyJson() {
  if (!exportedJson.value) return
  navigator.clipboard.writeText(exportedJson.value)
  ElMessage.success('已复制到剪贴板')
}

function downloadJson() {
  if (!exportedJson.value) return
  const blob = new Blob([exportedJson.value], { type: 'application/json' })
  triggerDownload(blob, `gateway-config-${Date.now()}.json`)
}

function onFile(file: UploadFile) {
  const reader = new FileReader()
  reader.onload = () => { importForm.json = String(reader.result) }
  if (file.raw) reader.readAsText(file.raw)
}

async function doImport() {
  importResult.value = null
  // 字段级校验：JSON 解析失败时错误显示在文本框下方，而不是只给一条汇总提示
  const formEl = importFormRef.value
  if (formEl) {
    const ok = await formEl.validate().catch(() => false)
    if (!ok) return
  }
  let parsed: GatewayConfig
  try {
    parsed = JSON.parse(importForm.json) as GatewayConfig
  } catch (e) {
    importResult.value = { success: false, error: 'JSON 解析失败：' + (extractErrorMessage(e)) }
    return
  }
  // 解析通过后不直接覆盖：先弹配置变更预览（内部调用 configDiff），
  // 用户确认后再走 confirmImport。旧实现解析完就 importConfig，导入内容写错了也没有反悔窗口。
  pendingImport.value = parsed
  showImportPreview.value = true
}

/** 预览里点「确认导入」后才真正提交 */
async function confirmImport() {
  if (!pendingImport.value) return
  importing.value = true
  try {
    await importConfig(pendingImport.value)
    importResult.value = { success: true }
    ElMessage.success('配置导入成功')
  } catch (e) {
    importResult.value = { success: false, error: extractErrorMessage(e) }
  } finally {
    importing.value = false
    pendingImport.value = null
  }
}

// 关闭预览时立即释放已解析的配置，避免「点了关闭但内存里还挂着一份待导入配置」
watch(showImportPreview, (open) => { if (!open) pendingImport.value = null })

// ===== 工程文件（总线 / 上行通道 / 变量映射 三层子集）=====

async function doExportProject() {
  exportingProject.value = true
  try {
    const project = await exportProject()
    projectJson.value = JSON.stringify(project, null, 2)
    ElMessage.success('工程文件已导出')
  } catch (e) {
    ElMessage.error('导出工程文件失败：' + extractErrorMessage(e))
  } finally {
    exportingProject.value = false
  }
}

function copyProjectJson() {
  if (!projectJson.value) return
  navigator.clipboard.writeText(projectJson.value)
  ElMessage.success('已复制到剪贴板')
}

function downloadProjectJson() {
  if (!projectJson.value) return
  const blob = new Blob([projectJson.value], { type: 'application/json' })
  triggerDownload(blob, `gateway-project-${Date.now()}.json`)
}

function onProjectFile(file: UploadFile) {
  const reader = new FileReader()
  reader.onload = () => { projectImportForm.json = String(reader.result) }
  if (file.raw) reader.readAsText(file.raw)
}

async function doImportProject() {
  projectImportResult.value = null
  // 字段级校验：JSON 解析失败时错误显示在文本框下方，而不是只给一条汇总提示
  const formEl = projectImportFormRef.value
  if (formEl) {
    const ok = await formEl.validate().catch(() => false)
    if (!ok) return
  }
  let parsed: ProjectFile
  try {
    parsed = JSON.parse(projectImportForm.json) as ProjectFile
  } catch (e) {
    projectImportResult.value = { success: false, error: 'JSON 解析失败：' + extractErrorMessage(e) }
    return
  }
  importingProject.value = true
  try {
    await importProject(parsed)
    projectImportResult.value = { success: true }
    ElMessage.success('工程文件导入成功')
  } catch (e) {
    projectImportResult.value = { success: false, error: extractErrorMessage(e) }
  } finally {
    importingProject.value = false
  }
}
</script>

<style scoped>
.io { padding: 16px; }
.section-card { margin-bottom: 16px; }
.json-box { margin-top: 12px; font-family: monospace; font-size: 0.75rem; }
.actions { margin-top: 12px; display: flex; gap: 8px; }
</style>