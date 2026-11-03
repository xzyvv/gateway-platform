<template>
  <div class="cfi">
    <SectionHeader title="片段导入" icon="Files" />

    <el-alert
      type="info"
      :closable="false"
      show-icon
      class="cfi-tip"
      title="只贴要变更的部分：总线、设备、点位、设备分组、虚拟点位、转换规则按主键合并进当前配置，其余配置节原样保留。"
    />

    <el-row :gutter="16">
      <el-col :span="10">
        <el-card shadow="never" class="cfi-card">
          <template #header>
            <div class="cfi-head">
              <strong>片段内容</strong>
              <div class="cfi-head-actions">
                <el-button size="small" link @click="fillSample">填入示例</el-button>
                <el-upload :auto-upload="false" :show-file-list="false" accept=".json,application/json" :on-change="onFileChange">
                  <el-button size="small" :icon="Upload">选择 JSON 文件</el-button>
                </el-upload>
              </div>
            </div>
          </template>
          <el-input
            v-model="text"
            type="textarea"
            :rows="16"
            resize="vertical"
            aria-label="待导入的配置片段"
            placeholder='例如 {"points": [{"id": "p_temp", "name": "温度"}]}'
          />
          <el-button type="primary" style="width:100%;margin-top:12px" :icon="CircleCheck" :loading="validating" @click="validate">开始校验</el-button>
          <p class="cfi-hint">校验规则由网关后端统一提供（与「配置体检」「导入导出」同一套），前端不另立标准。</p>
        </el-card>
      </el-col>

      <el-col :span="14">
        <el-card v-if="statRows.length && !done" shadow="never" class="cfi-card">
          <template #header>
            <div class="cfi-head">
              <strong>校验结果</strong>
              <el-tag :type="checkTagType" effect="dark">{{ checkTagText }}</el-tag>
            </div>
          </template>

          <el-alert
            v-if="blocked"
            type="warning"
            :closable="false"
            show-icon
            title="当前网关已配置设备模板，新增点位不会参与采集"
            description="运行期点位由设备模板展开，只导入点位会「提示成功但不生效」。请改用「设备管理」调整模板，或用「导入导出」页整份导入。"
          />

          <div class="cfi-stat">
            <DataTable :data="statRows" :columns="statColumns" row-key="key" empty-text="片段未包含任何配置节" />
          </div>

          <h4 class="cfi-sub">后端校验：{{ errorCount }} 处错误 / {{ warningCount }} 处警告</h4>
          <DataTable :data="issueRows" :columns="issueColumns" row-key="key" stripe empty-text="未发现问题" />

          <el-button
            type="success"
            style="width:100%;margin-top:12px"
            :disabled="!canCommit"
            :loading="committing"
            :icon="Check"
            @click="commit"
          >确认导入</el-button>

          <ImportOutcome :visible="!!commitError" :success="false" :error="commitError" style="margin-top:12px" />
        </el-card>

        <el-card v-else-if="!done" shadow="never" class="cfi-card">
          <EmptyState description="粘贴或上传片段后点「开始校验」，这里会显示各配置节的新增/覆盖数量与后端校验问题" />
        </el-card>

        <el-card v-else shadow="never" class="cfi-card">
          <ImportOutcome
            :visible="done !== null"
            :success="true"
            success-title="片段已合并导入"
            :subtitle="`新增 ${done?.added ?? 0} 条、覆盖 ${done?.updated ?? 0} 条，配置已热重载；本次变更已写入配置版本，可随时回滚。`"
          >
            <template #extra>
              <el-button type="primary" @click="goVersions">查看配置版本</el-button>
              <el-button @click="resetAll">继续导入</el-button>
            </template>
          </ImportOutcome>
        </el-card>
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
/** 配置片段导入：只在现有配置上按主键合并 6 类列表节，不做整份覆盖。
 *  校验走后端 `POST /api/config/validate`，提交走 `POST /api/config`（锁内校验 + 热重载 + 版本快照）。
 *  与「导入导出」页的分工：那边整份覆盖（迁移/恢复），这边增量合并（现场贴一段）。
 */
import { computed, h, ref, toRaw } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElTag, type UploadFile } from 'element-plus'
import { Check, CircleCheck, Upload } from '@element-plus/icons-vue'
import DataTable from '@/components/DataTable.vue'
import EmptyState from '@/components/EmptyState.vue'
import ImportOutcome from '@/components/ImportOutcome.vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { getConfig, postConfig, validateConfigDraft, type ConfigValidationIssue } from '@/api/config'
import type { GatewayConfig } from '@/api/types'
import { extractErrorMessage } from '@/utils/errorHandler'
import {
  SECTION_LABELS,
  mergeFragment,
  parseFragment,
  pointsIgnoredByTemplateMode,
  type SectionStat,
} from '@/utils/configFragment'

const SAMPLE = JSON.stringify(
  {
    points: [
      { id: 'p_temp', name: '温度', bus: 'com1', protocol: 'modbus', address: '40001', interval: 5 },
      { id: 'p_humi', name: '湿度', bus: 'com1', protocol: 'modbus', address: '40002', interval: 5 },
    ],
  },
  null,
  2,
)

const router = useRouter()
const text = ref('')
const current = ref<GatewayConfig | null>(null)
const merged = ref<GatewayConfig | null>(null)
const stats = ref<SectionStat[]>([])
const issues = ref<ConfigValidationIssue[]>([])
const errorCount = ref(0)
const warningCount = ref(0)
const blocked = ref(false)
const validating = ref(false)
const committing = ref(false)
const done = ref<{ added: number; updated: number } | null>(null)
// 提交失败原因：行内展示（与「导入导出」同一套结果面板），不依赖一闪而过的 toast
const commitError = ref('')

const statRows = computed(() => stats.value.map((s) => ({ ...s, key: s.section, label: SECTION_LABELS[s.section] })))
const issueRows = computed(() => issues.value.map((i, idx) => ({ ...i, key: `${idx}-${i.field}` })))
const totalChanges = computed(() => stats.value.reduce((n, s) => n + s.added + s.updated, 0))
const canCommit = computed(() => errorCount.value === 0 && totalChanges.value > 0 && !blocked.value)

const checkTagType = computed(() => {
  if (errorCount.value > 0) return 'danger'
  return totalChanges.value > 0 ? 'success' : 'info'
})
const checkTagText = computed(() => {
  if (errorCount.value > 0) return `存在 ${errorCount.value} 处错误`
  if (totalChanges.value === 0) return '与当前配置完全一致'
  return `可导入：新增 / 覆盖 ${totalChanges.value} 条`
})

type StatRow = SectionStat & { key: string; label: string }

const statColumns: ColumnDef<StatRow>[] = [
  { key: 'label', title: '配置节', width: 110 },
  { key: 'added', title: '新增', width: 70, align: 'right', cell: (row) => String(row.added) },
  { key: 'updated', title: '覆盖', width: 70, align: 'right', cell: (row) => String(row.updated) },
  { key: 'unchanged', title: '未变', width: 70, align: 'right', cell: (row) => String(row.unchanged) },
]

const issueColumns: ColumnDef<ConfigValidationIssue & { key: string }>[] = [
  {
    key: 'level',
    title: '级别',
    width: 80,
    cell: (row) => h(ElTag, { size: 'small', type: row.level === 'error' ? 'danger' : 'warning' }, () => (row.level === 'error' ? '错误' : '警告')),
  },
  {
    key: 'field',
    title: '字段',
    width: 200,
    cell: (row) =>
      row.field
        ? h('code', { style: 'font-size:12px' }, row.field)
        : h('span', { style: 'color:var(--el-text-color-placeholder)' }, '—'),
  },
  { key: 'message', title: '说明' },
]

function fillSample() {
  text.value = SAMPLE
}

function onFileChange(file: UploadFile) {
  const raw = file.raw
  if (!raw) return
  const reader = new FileReader()
  reader.onload = () => {
    text.value = String(reader.result ?? '')
    ElMessage.success('已读取文件内容，可点击「开始校验」')
  }
  reader.onerror = () => ElMessage.error('文件读取失败，请确认是 UTF-8 编码的 JSON')
  reader.readAsText(raw)
}

function resetResult() {
  merged.value = null
  stats.value = []
  issues.value = []
  errorCount.value = 0
  warningCount.value = 0
  blocked.value = false
  commitError.value = ''
}

async function validate() {
  done.value = null
  resetResult()

  const parsed = parseFragment(text.value)
  if (!parsed.ok) {
    ElMessage.error(parsed.error)
    return
  }

  validating.value = true
  try {
    if (!current.value) current.value = await getConfig()
    // 传入 raw 对象：mergeFragment 内部用 structuredClone 深拷贝，Vue 的响应式 Proxy 不可结构化克隆
    const currentConfig = toRaw(current.value)
    const { merged: mergedConfig, stats: sectionStats } = mergeFragment(currentConfig, parsed.fragment)
    merged.value = mergedConfig
    stats.value = sectionStats
    blocked.value = pointsIgnoredByTemplateMode(currentConfig, parsed.fragment)

    const report = await validateConfigDraft(mergedConfig)
    issues.value = report.issues
    errorCount.value = report.errors
    warningCount.value = report.warnings
  } catch (e) {
    ElMessage.error(`校验失败：${extractErrorMessage(e)}`)
    resetResult()
  } finally {
    validating.value = false
  }
}

async function commit() {
  if (!merged.value) return
  commitError.value = ''
  committing.value = true
  try {
    await postConfig(merged.value)
    done.value = {
      added: stats.value.reduce((n, s) => n + s.added, 0),
      updated: stats.value.reduce((n, s) => n + s.updated, 0),
    }
    ElMessage.success('片段已合并导入')
    current.value = null
    resetResult()
    text.value = ''
  } catch (e) {
    commitError.value = extractErrorMessage(e)
  } finally {
    committing.value = false
  }
}

function resetAll() {
  done.value = null
  resetResult()
  text.value = ''
}

function goVersions() {
  router.push({ path: '/system/config', query: { tab: 'config-versions' } })
}
</script>

<style scoped>
.cfi-tip { margin-bottom: 12px; }
.cfi-card { height: 100%; }
.cfi-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.cfi-head-actions { display: flex; align-items: center; gap: 8px; }
.cfi-hint { margin: 8px 0 0; font-size: 12px; color: var(--el-text-color-secondary); line-height: 1.5; }
.cfi-stat { margin: 12px 0; }
.cfi-sub { margin: 0 0 8px; font-size: 13px; color: var(--el-text-color-regular); }
</style>
