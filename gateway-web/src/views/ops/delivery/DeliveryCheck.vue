<template>
  <div class="dc">
    <SectionHeader title="交付验收">
      <template #extra>
        <el-tag v-if="result" :type="LEVEL_TAG_TYPE[result.level]" effect="dark" size="small">
          自检{{ LEVEL_LABEL[result.level] }}
        </el-tag>
      </template>
      <el-button size="small" :loading="checking" @click="handleSelfCheck">一键自检</el-button>
    </SectionHeader>

    <el-alert type="info" :closable="false" class="guide">
      交付顺序：套用默认配置模板 → 按现场改串口/IP 与地址 → 一键自检六项 → 勾选人工确认项 → 导出交付检查单。
      模板只覆盖「总线 / 北向 / 变量映射」三层，不会改动本机 Web 鉴权与时钟配置。
    </el-alert>

    <!-- 1. 默认配置模板 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <div class="card-head">
          <strong>默认配置模板</strong>
          <span class="sub">初始工程模板，下载后可在「系统设置 → 配置管理 → 导入导出 → 工程文件」导入</span>
        </div>
      </template>
      <div v-for="tpl in TEMPLATES" :key="tpl.file" class="tpl-row">
        <div class="tpl-main">
          <div class="tpl-name">{{ tpl.name }}</div>
          <div class="tpl-desc">{{ tpl.desc }}</div>
        </div>
        <div class="tpl-actions">
          <el-button size="small" @click="downloadTemplate(tpl)">下载 JSON</el-button>
          <el-button size="small" type="primary" plain :loading="applying === tpl.file" @click="applyTemplate(tpl)">
            应用模板
          </el-button>
        </div>
      </div>
      <el-alert type="warning" :closable="false" class="tpl-warn">
        应用模板会覆盖当前的总线、北向与变量映射，并默认关闭 MQTT；现场请先导出工程备份，再按实际串口/IP、从站地址与点表修改。
      </el-alert>
    </el-card>

    <!-- 2. 一键自检 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <div class="card-head">
          <strong>一键自检</strong>
          <span class="sub">协议 / 总线 / 北向 / 时钟 / 证书 / 看门狗六项，复用现有只读接口</span>
        </div>
      </template>

      <EmptyState v-if="!result" description="尚未自检，点击右上角「一键自检」" :image-size="64" />
      <template v-else>
        <DataTable :data="result.items" :columns="checkColumns" row-key="id" empty-text="暂无检查项" />
        <div class="report">
          <div class="report-head">
            <span>自检报告（Markdown，可复制给研发或存档）</span>
            <span class="report-actions">
              <el-button size="small" @click="copyReport">复制报告</el-button>
              <el-button size="small" @click="downloadSelfCheckMd">下载报告</el-button>
            </span>
          </div>
          <el-input
            :model-value="reportText"
            type="textarea"
            :rows="9"
            readonly
            resize="none"
            aria-label="自检报告文本"
          />
        </div>
      </template>
    </el-card>

    <!-- 3. 交付检查单 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <div class="card-head">
          <strong>交付检查单</strong>
          <span class="sub">自动自检结果 + 现场人工确认，可导出 JSON / Markdown</span>
        </div>
      </template>

      <div class="meta-row">
        <span class="meta-label">交付人</span>
        <el-input
          v-model="operator"
          size="small"
          class="operator"
          placeholder="填写现场工程师姓名"
          aria-label="交付人"
          clearable
        />
        <span class="meta-label">总体结论</span>
        <el-tag :type="LEVEL_TAG_TYPE[checklist.overall === 'todo' ? 'warn' : checklist.overall]" effect="light">
          {{ STATUS_LABEL[checklist.overall] }}
        </el-tag>
        <span class="meta-hint">「待确认」表示自检未执行或人工项未勾选</span>
      </div>

      <div class="manual">
        <div v-for="def in MANUAL_ITEMS" :key="def.id" class="manual-row">
          <el-checkbox v-model="manualOf(def.id).checked" class="manual-check">{{ def.title }}</el-checkbox>
          <span class="manual-hint">{{ def.hint }}</span>
          <el-input
            v-model="manualOf(def.id).note"
            size="small"
            class="manual-note"
            :aria-label="`${def.title}备注`"
            placeholder="备注（可选）"
            clearable
          />
        </div>
      </div>

      <div class="export-actions">
        <el-button size="small" type="primary" plain @click="exportChecklistJson">导出 JSON</el-button>
        <el-button size="small" @click="exportChecklistMd">导出 Markdown</el-button>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
/**
 * 交付验收页签（P2 交付验收包）
 *
 * 三件事：
 * 1. 默认工程模板：下载/一键应用 Modbus 电力仪表、IEC104 变电站两套初始配置；
 * 2. 一键自检：并行调用现有只读接口，汇总协议/总线/北向/时钟/证书/看门狗六项，
 *    输出可复制的 Markdown 报告；
 * 3. 交付检查单：六项自检 + 人工确认项，导出 JSON / Markdown 存档。
 *
 * 不新增后端聚合 API，数据源均为已有端点，避免与健康报告重复实现判定口径。
 */
import { computed, h, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox, ElTag } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { getVersion } from '@/api/status'
import { importProject, type ProjectFile } from '@/api/project'
import type { VersionInfo } from '@/api/types'
import { copyText } from '@/utils/clipboard'
import { triggerDownload } from '@/utils/exportCsv'
import { useHealthReport } from '@/composables/system/useHealthReport'
import {
  LEVEL_LABEL,
  LEVEL_TAG_TYPE,
  MANUAL_ITEMS,
  buildChecklist,
  buildSelfCheckMarkdown,
  checklistToMarkdown,
  runSelfCheck,
  type CheckItem,
  type SelfCheckResult,
} from './selfCheck'

interface TemplateDef {
  name: string
  file: string
  desc: string
}

/** 模板文件放在 gateway-web/public/delivery-templates，随前端静态资源一起发布 */
const TEMPLATES: TemplateDef[] = [
  {
    name: 'Modbus 电力仪表',
    file: 'modbus-power-meter.json',
    desc: '1 路 Modbus-RTU 串口 + 1 台三相多功能电表（12 个遥测点），默认 COM3 / 从站 1 / 5s。',
  },
  {
    name: 'IEC104 变电站',
    file: 'iec104-station.json',
    desc: '1 路 IEC104 TCP 站（12 个遥测遥信点，IOA 1001~1010 / 2001~2002），默认 192.168.1.10:2404。',
  },
]

const version = ref<VersionInfo | null>(null)
const result = ref<SelfCheckResult | null>(null)
const checking = ref(false)
const applying = ref('')
const operator = ref('')
const manual = reactive<Record<string, { checked: boolean; note: string }>>(
  Object.fromEntries(MANUAL_ITEMS.map((i) => [i.id, { checked: false, note: '' }])),
)

/** 索引访问在 noUncheckedIndexedAccess 下可能为空，统一在此兜底 */
function manualOf(id: string): { checked: boolean; note: string } {
  const state = manual[id] ?? { checked: false, note: '' }
  manual[id] = state
  return state
}

// 应用模板后让健康报告下一次进入重新拉取（10s TTL）
const { refresh } = useHealthReport()

const checkColumns: ColumnDef<CheckItem>[] = [
  { key: 'title', title: '检查项', width: 110 },
  {
    key: 'level',
    title: '结论',
    width: 90,
    cell: (row) =>
      h(ElTag, { size: 'small', type: LEVEL_TAG_TYPE[row.level], effect: 'light' }, () =>
        LEVEL_LABEL[row.level],
      ),
  },
  { key: 'detail', title: '说明', minWidth: 320 },
]

const checklist = computed(() => buildChecklist(result.value, manual, operator.value, version.value))
const reportText = computed(() =>
  result.value ? buildSelfCheckMarkdown(result.value, version.value) : '',
)

const STATUS_LABEL = {
  pass: '通过',
  warn: '提醒（可交付）',
  fail: '未通过',
  todo: '待确认',
} as const

onMounted(async () => {
  try {
    version.value = await getVersion()
  } catch {
    version.value = null
  }
})

async function handleSelfCheck() {
  checking.value = true
  try {
    result.value = await runSelfCheck()
    const { level, items } = result.value
    if (level === 'pass') ElMessage.success('自检通过，可以交付')
    else if (level === 'warn') ElMessage.warning('自检有提醒项，请按说明确认后再交付')
    else ElMessage.error(`自检未通过：${items.filter((i) => i.level === 'fail').map((i) => i.title).join('、')}`)
  } catch (e) {
    ElMessage.error(`自检执行失败：${e instanceof Error ? e.message : String(e)}`)
  } finally {
    checking.value = false
  }
}

function templateUrl(tpl: TemplateDef): string {
  return `${import.meta.env.BASE_URL}delivery-templates/${tpl.file}`
}

async function fetchTemplate(tpl: TemplateDef): Promise<ProjectFile> {
  const res = await fetch(templateUrl(tpl))
  if (!res.ok) throw new Error(`下载模板失败：HTTP ${res.status}`)
  return (await res.json()) as ProjectFile
}

async function downloadTemplate(tpl: TemplateDef) {
  try {
    const data = await fetchTemplate(tpl)
    triggerDownload(
      new Blob([JSON.stringify(data, null, 2)], { type: 'application/json;charset=utf-8' }),
      tpl.file,
    )
    ElMessage.success('模板已下载')
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : String(e))
  }
}

async function applyTemplate(tpl: TemplateDef) {
  try {
    await ElMessageBox.confirm(
      `将用「${tpl.name}」覆盖当前的总线、北向与变量映射三层配置（不改 Web 鉴权/时钟）。建议先导出工程备份，是否继续？`,
      '应用配置模板',
      { type: 'warning', confirmButtonText: '覆盖并应用', cancelButtonText: '取消' },
    )
  } catch {
    return
  }
  applying.value = tpl.file
  try {
    const data = await fetchTemplate(tpl)
    await importProject(data)
    refresh(true)
    ElMessage.success('模板已应用，请点击「一键自检」确认采集与链路状态')
  } catch (e) {
    ElMessage.error(`应用模板失败：${e instanceof Error ? e.message : String(e)}`)
  } finally {
    applying.value = ''
  }
}

async function copyReport() {
  await copyText(reportText.value, '自检报告已复制')
}

function fileStamp(): string {
  const d = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`
}

function downloadSelfCheckMd() {
  if (!result.value) return
  triggerDownload(
    new Blob([reportText.value], { type: 'text/markdown;charset=utf-8' }),
    `gateway-self-check-${fileStamp()}.md`,
  )
}

function exportChecklistJson() {
  triggerDownload(
    new Blob([JSON.stringify(checklist.value, null, 2)], { type: 'application/json;charset=utf-8' }),
    `gateway-delivery-checklist-${fileStamp()}.json`,
  )
}

function exportChecklistMd() {
  triggerDownload(
    new Blob([checklistToMarkdown(checklist.value)], { type: 'text/markdown;charset=utf-8' }),
    `gateway-delivery-checklist-${fileStamp()}.md`,
  )
}
</script>

<style scoped>
.dc {
  padding: 16px;
}
.guide {
  margin-bottom: 16px;
}
.section-card {
  margin-bottom: 16px;
}
.card-head {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 8px;
}
.sub {
  color: var(--text-secondary);
  font-size: 0.78rem;
}
.tpl-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 0;
  border-bottom: 1px solid var(--border-lighter);
}
.tpl-row:last-of-type {
  border-bottom: none;
}
.tpl-main {
  flex: 1;
  min-width: 0;
}
.tpl-name {
  font-weight: 600;
}
.tpl-desc {
  color: var(--text-secondary);
  font-size: 0.8rem;
  margin-top: 2px;
}
.tpl-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}
.tpl-warn {
  margin-top: 10px;
}
.report {
  margin-top: 14px;
}
.report-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 6px;
  color: var(--text-secondary);
  font-size: 0.82rem;
}
.report-actions {
  display: flex;
  gap: 8px;
}
.meta-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px;
  margin-bottom: 12px;
}
.meta-label {
  color: var(--text-secondary);
  font-size: 0.82rem;
}
.operator {
  width: 180px;
}
.meta-hint {
  color: var(--text-placeholder);
  font-size: 0.76rem;
}
.manual {
  border: 1px solid var(--border-lighter);
  border-radius: var(--radius-small);
  padding: 6px 12px;
}
.manual-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 0;
  border-bottom: 1px dashed var(--border-lighter);
}
.manual-row:last-child {
  border-bottom: none;
}
.manual-check {
  width: 150px;
  flex-shrink: 0;
}
.manual-hint {
  flex: 1;
  color: var(--text-secondary);
  font-size: 0.8rem;
}
.manual-note {
  width: 220px;
  flex-shrink: 0;
}
.export-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 12px;
}
</style>
