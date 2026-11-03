<template>
  <div class="ch">
    <SectionHeader title="配置体检" icon="FirstAidKit" />

    <!-- P0 修复：空配置不能给满分。原实现所有检查项都是「遍历已有条目」的条件式，
         空配置时循环零次 → 0 条问题 → 评分 100，用户会以为「配置健康、一切正常」，
         实际连一条总线都没有。这里显式区分「没东西可查」与「查了没问题」。 -->
    <el-card v-if="hasRun && isEmpty" shadow="never" class="section-card">
      <el-result
        icon="info"
        title="尚未配置任何内容，无法体检"
        sub-title="当前网关没有总线、设备或点位，体检没有可检查的对象。请先完成接入配置，再回来检查。"
      >
        <template #extra>
          <el-button type="primary" @click="goBusConfig">去新增总线</el-button>
          <el-button @click="goPoints">去配置点位</el-button>
          <el-button plain @click="run" :icon="Refresh" :loading="loading">重新检查</el-button>
        </template>
      </el-result>
    </el-card>

    <el-card v-else shadow="never" class="section-card">
      <div class="score-bar">
        <el-progress type="circle" :percentage="summary.score" :width="100" :color="scoreColor" />
        <div class="summary-text">
          <h3 v-if="hasRun">配置健康评分 {{ summary.score }} 分</h3>
          <h3 v-else>配置健康检查</h3>
          <p v-if="hasRun">检测到 <b class="err">{{ summary.errors }} 个错误</b>，<b class="warn">{{ summary.warnings }} 个警告</b>，{{ summary.infos }} 个提示</p>
          <p v-else>检查配置中的矛盾项与高风险写法（重名、重复地址、模板缺失、超时越界等）</p>
          <el-button type="primary" @click="run" :icon="Refresh" :loading="loading">{{ hasRun ? '重新检查' : '开始检查' }}</el-button>
        </div>
      </div>
    </el-card>

    <el-card v-if="hasRun && !isEmpty" shadow="never" class="section-card">
      <template #header><strong>问题清单</strong>（共 {{ issues.length }} 项）</template>
      <DataTable
        :data="issues"
        :columns="healthColumns"
        row-key="id"
        stripe
        empty-text="暂无问题"
      />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, h } from 'vue'
import { useRouter } from 'vue-router'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElButton, ElMessage, ElTag } from 'element-plus'
import { Refresh } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import { getConfig } from '@/api/config'
import { listTemplates } from '@/api/templates'
import type { GatewayConfig } from '@/api/types'
// 体检规则单源在后端，这里只做「后端 issue → UI 行」适配；总览面板共用同一适配层
import { loadConfigIssues, type ConfigIssue } from '@/utils/configHealth'

// 问题清单列定义，顺序/宽度与原 el-table 保持一致
const healthColumns: ColumnDef<ConfigIssue>[] = [
  {
    key: 'level',
    title: '级别',
    width: 80,
    cell: (row) =>
      h(
        ElTag,
        {
          size: 'small',
          effect: 'dark',
          round: true,
          type: row.level === 'error' ? 'danger' : row.level === 'warning' ? 'warning' : 'info',
        },
        () => (row.level === 'error' ? '错误' : row.level === 'warning' ? '警告' : '提示'),
      ),
  },
  { key: 'category', title: '类别', width: 100 },
  { key: 'target', title: '对象', width: 160 },
  { key: 'description', title: '问题描述', minWidth: 240, ellipsis: true },
  { key: 'suggestion', title: '修复建议', minWidth: 200, ellipsis: true },
  actionColumn<ConfigIssue>({
    key: 'action',
    width: 100,
    cell: (row) =>
      row.autoFixable
        ? h(ElButton, { size: 'small', type: 'success', link: true, onClick: () => fix(row) }, () => '一键修复')
        : h('span', { class: 'manual' }, '需手动处理'),
  }),
]

const issues = ref<ConfigIssue[]>([])
const summary = ref({ total: 0, errors: 0, warnings: 0, infos: 0, score: 0 })
const loading = ref(false)
const hasRun = ref(false)
// P0: 完全空配置时不给评分（否则 0 条问题 → 100 分，等于告诉用户「一切正常」）
const isEmpty = ref(false)
const router = useRouter()
let currentConfig: GatewayConfig | null = null
let currentTemplateNames: Set<string> = new Set()

function goBusConfig() {
  router.push({ path: '/system/config', query: { tab: 'runtime-config' } })
}
function goPoints() {
  router.push({ path: '/points', query: { tab: 'points' } })
}

const scoreColor = computed(() => {
  const s = summary.value.score
  return s >= 80 ? 'var(--el-color-success)' : s >= 50 ? 'var(--el-color-warning)' : 'var(--el-color-danger)'
})


async function run() {
  loading.value = true
  try {
    const [cfg, tpls] = await Promise.all([
      getConfig(),
      // S6: 模板库拉取失败不阻塞体检，退化为跳过模板类检查
      listTemplates().catch(() => [] as { name: string }[]),
    ])
    currentConfig = cfg
    const templateNames = new Set(tpls.map((t) => t.name))
    currentTemplateNames = templateNames
    isEmpty.value = (cfg.buses?.length ?? 0) === 0 && (cfg.devices?.length ?? 0) === 0 && (cfg.points?.length ?? 0) === 0
    // A3：规则在后端，模板清单随请求一起下发（后端据此判定模板存在性）
    const list = await loadConfigIssues(cfg, templateNames)
    issues.value = list
    const errors = list.filter((i) => i.level === 'error').length
    const warnings = list.filter((i) => i.level === 'warning').length
    const infos = list.filter((i) => i.level === 'info').length
    const score = Math.max(0, 100 - errors * 15 - warnings * 5 - infos * 1)
    summary.value = { total: list.length, errors, warnings, infos, score }
    hasRun.value = true
    if (isEmpty.value) ElMessage.warning('尚未配置任何内容，无法体检')
    else ElMessage.success('检查完成')
  } catch (e) { showToast(e, '检查失败') }
  finally { loading.value = false }
}

async function fix(row: ConfigIssue) {
  if (!row.fix) return
  try {
    await row.fix()
    row.autoFixable = false
    ElMessage.success(`已自动修复：${row.description}`)
    if (currentConfig) issues.value = await loadConfigIssues(currentConfig, currentTemplateNames)
  } catch (e) { showToast(e, '修复失败') }
}
</script>

<style scoped>
.ch { padding: 16px; }
.section-card { margin-bottom: 16px; }
.score-bar { display: flex; align-items: center; gap: 40px; }
.summary-text h3 { margin: 0 0 8px; }
.summary-text p { margin: 0 0 12px; color: var(--el-text-color-secondary); }
.summary-text .err { color: var(--text-danger); }
.summary-text .warn { color: var(--text-warning); }
.manual { color: var(--el-text-color-secondary); font-size: 0.75rem; }
</style>
