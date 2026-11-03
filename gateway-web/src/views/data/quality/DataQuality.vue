<template>
  <div class="dq">
    <!-- 页面级 h1 已由 HubShell 渲染「质量概览」，这里只标注下方四张卡片的含义 -->
    <SectionHeader title="质量等级分布" icon="DataLine">
      <template #extra>
        <el-button size="small" @click="goCommProfile">设备级通信成功率（通信画像）</el-button>
        <el-tag v-if="rulesSource === 'remote'" type="success" size="small" effect="plain">
          远端已落库 · 规则已下发网关执行
        </el-tag>
        <LocalOnlyBadge
          v-else
          label="规则仅本机"
          tip="「质量规则配置」里的规则只保存在当前浏览器的 localStorage，网关不加载、不执行，也不会影响上行数据；网关实际使用的质量判定见「点位质量实时状态」与「设备健康评分」。"
        />
      </template>
    </SectionHeader>

    <el-alert
      type="info"
      :closable="false"
      show-icon
      title="本页是点位级明细：质量码与「设备健康评分」/「设备排行」使用同一套判定规则（good / stale→不确定 / 连续失败≥3→坏 / 其余→过期）。设备综合分请见设备健康评分。"
      style="margin-bottom:12px"
    />

    <el-row :gutter="16">
      <el-col :span="6">
        <el-card shadow="never" class="stat">
          <div class="lbl">{{ QUALITY_LABEL.good }}</div>
          <div class="val good">{{ stats.good }}</div>
        </el-card>
      </el-col>
      <el-col :span="6">
        <el-card shadow="never" class="stat">
          <div class="lbl">{{ QUALITY_LABEL.uncertain }}</div>
          <div class="val uncertain">{{ stats.stale }}</div>
        </el-card>
      </el-col>
      <el-col :span="6">
        <el-card shadow="never" class="stat">
          <div class="lbl">{{ QUALITY_LABEL.old }}</div>
          <div class="val old">{{ stats.unknown }}</div>
        </el-card>
      </el-col>
      <el-col :span="6">
        <el-card shadow="never" class="stat">
          <div class="lbl">{{ QUALITY_LABEL.bad }}</div>
          <div class="val bad">{{ stats.bad }}</div>
        </el-card>
      </el-col>
    </el-row>

    <el-row :gutter="16">
      <el-col :span="11">
        <el-card shadow="never" class="section-card">
          <template #header><strong>质量规则配置</strong></template>
          <div class="top-bar">
            <el-button type="primary" size="small" @click="openCreate"><el-icon><Plus /></el-icon> 新建</el-button>
            <el-button size="small" @click="loadRules">刷新</el-button>
          </div>
          <DataTable :data="rules" :columns="ruleColumns" row-key="id" stripe :max-height="tableMaxHeight" empty-text="暂无规则" />
        </el-card>
      </el-col>
      <el-col :span="14">
        <el-card shadow="never" class="section-card">
          <template #header><strong>点位质量实时状态</strong> <el-button link @click="loadPoints">刷新</el-button></template>
          <DataTable :data="points" :columns="pointColumns" row-key="point_id" stripe :max-height="tableMaxHeight" />
        </el-card>
      </el-col>
    </el-row>

    <el-dialog :close-on-click-modal="false" v-model="showForm" title="新建质量规则" width="480px" @closed="resetForm" :before-close="formGuard.beforeClose">
      <el-form label-width="100px">
        <el-form-item label="规则名称"><el-input v-model="form.name" /></el-form-item>
        <el-form-item label="检查条件">
          <el-select v-model="form.condition" style="width:100%">
            <el-option label="取值范围检查" value="range" />
            <el-option label="变化率检查" value="rate" />
            <el-option label="超时未更新" value="stale" />
          </el-select>
        </el-form-item>
        <el-form-item v-if="form.condition === 'range'" label="合法范围">
          <el-input-number v-model="form.min" controls-position="right" style="width:120px" /> ~
          <el-input-number v-model="form.max" controls-position="right" style="width:120px" />
        </el-form-item>
        <el-form-item v-if="form.condition === 'rate'" label="最大变化率"><el-input-number v-model="form.maxRate" controls-position="right" /></el-form-item>
        <el-form-item v-if="form.condition === 'stale'" label="超时(秒)"><el-input-number v-model="form.staleSeconds" :min="1" controls-position="right" /></el-form-item>
        <el-form-item label="降级为">
          <el-select v-model="form.resultQuality" style="width:100%">
            <el-option v-for="(l,k) in QUALITY_LABEL" :key="k" :label="l" :value="k" />
          </el-select>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="formGuard.close">取消</el-button>
        <el-button type="primary" :loading="submitting" :disabled="submitting" @click="save">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted, h } from 'vue'
import { useRouter } from 'vue-router'
import { ElButton, ElMessage, ElSwitch, ElTag } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import { Plus } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import {
  listQualityRules, createQualityRule, deleteQualityRule, updateQualityRule,
  getQualityStats, qualityRulesSource,
  QUALITY_LABEL, QUALITY_COLOR,
  type QualityRule, type PointQuality, type Quality, type QualityStats, type QualityRulesSource,
} from '@/api/dataQuality'
import { usePointQuality } from '@/composables/data/usePointQuality'
import { formatTimeOfDay } from '@/utils/datetime'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'

const rules = ref<QualityRule[]>([])
/** 规则数据来源：remote = 已落库网关并下发执行；local = 后端不可用时的本机回退 */
const rulesSource = ref<QualityRulesSource>('local')
const stats = ref<QualityStats>({ good: 0, bad: 0, stale: 0, unknown: 0, total: 0 })
// 点位质量快照统一走 usePointQuality（单一数据入口）
const { qualities: points, load: loadQualities } = usePointQuality()
/**
 * S9: 两张明细表的可视高度跟随视口收缩。
 * 原固定 420px，在 1366×768 上叠加头部与统计卡后整页要滚动 439px。
 * 这里留出统计卡与卡片头的高度（约 400px），高度不足时不低于 220px 仍可滚动查看。
 */
const tableMaxHeight = 'max(220px, calc(100vh - 400px))'
const showForm = ref(false)
/** 规则保存中：网关写接口往返期间禁用保存按钮，避免重复提交生成多条规则 */
const submitting = ref(false)
const { showToast } = useErrorDisplay()
const router = useRouter()

/**
 * 设备级通信成功率不在本页呈现（它是「采集成功率」口径，不是点位质量判定口径），
 * 统一跳到「采集 › 采集调优 › 通信画像」查看——原「质量分布」页签已下线。
 */
function goCommProfile() {
  router.push('/acquisition/comm-tuning?tab=comm-profile')
}
const form = reactive({ name: '', condition: 'range', min: 0, max: 100, maxRate: 100, staleSeconds: 60, resultQuality: 'bad' as Quality })

// 弹窗未保存修改守卫：X/ESC/取消关闭前对比基线，有改动先确认
const formGuard = useDialogDirtyGuard(showForm, () => form)

function qLabel(q: string): string { return QUALITY_LABEL[q as Quality] ?? String(q) }
function qColor(q: string): string { return QUALITY_COLOR[q as Quality] ?? '' }

/** QUALITY_COLOR 返回 string，h() 渲染 el-tag 需收窄为具体标签色 */
type TagType = 'primary' | 'success' | 'info' | 'warning' | 'danger'

// 表格统一走 DataTable（A4）；两张表行数均有限，默认走 el-table，超阈值自动切虚拟滚动
const ruleColumns: ColumnDef<QualityRule>[] = [
  { key: 'name', title: '名称', minWidth: 180, ellipsis: true },
  { key: 'condition', title: '条件', width: 100 },
  {
    key: 'resultQuality',
    title: '结果质量',
    width: 120,
    cell: (row) => h(ElTag, { size: 'small', type: qColor(row.resultQuality) as TagType }, () => qLabel(row.resultQuality)),
  },
  {
    key: 'enabled',
    title: '状态',
    width: 70,
    cell: (row) =>
      h(ElSwitch, {
        modelValue: row.enabled,
        size: 'small',
        'onUpdate:modelValue': (v: unknown) => { row.enabled = Boolean(v) },
        onChange: () => toggleRule(row),
      }),
  },
  actionColumn<QualityRule>({
    width: 70,
    cell: (row) => h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => remove(row) }, () => '删除'),
  }),
]

const pointColumns: ColumnDef<PointQuality>[] = [
  { key: 'point_name', title: '点位', width: 120 },
  { key: 'device_name', title: '设备', width: 120 },
  { key: 'value', title: '值', width: 90 },
  {
    key: 'quality',
    title: '质量',
    width: 120,
    cell: (row) => h(ElTag, { size: 'small', type: qColor(row.quality) as TagType }, () => qLabel(row.quality)),
  },
  { key: 'consecutive_failures', title: '连续失败', width: 90 },
  { key: 'last_update_at', title: '更新时间', width: 160, cell: (row) => formatTimeOfDay(row.last_update_at) },
]

async function loadRules() {
  try {
    rules.value = await listQualityRules()
    rulesSource.value = qualityRulesSource()
  } catch (e) {
    showToast(e, '加载质量规则失败')
  }
}
async function loadPoints() {
  try {
    await loadQualities()
    stats.value = await getQualityStats()
  } catch (e) {
    showToast(e, '加载质量状态失败')
  }
}
function resetForm() {
  Object.assign(form, { name: '', condition: 'range', min: 0, max: 100, maxRate: 100, staleSeconds: 60, resultQuality: 'bad' })
}
function openCreate() { resetForm(); showForm.value = true }
async function toggleRule(row: QualityRule) {
  try {
    await updateQualityRule(row.id, { enabled: row.enabled })
    rulesSource.value = qualityRulesSource()
  } catch (e) {
    // 落库失败时把开关拨回去：否则界面显示「已启用」，网关实际仍按旧配置执行
    row.enabled = !row.enabled
    showToast(e, '切换规则状态失败')
  }
}
async function save() {
  if (!form.name.trim()) { ElMessage.warning('请输入规则名称'); return }
  const payload: Omit<QualityRule, 'id'> = {
    name: form.name, condition: form.condition as QualityRule['condition'],
    min: form.min, max: form.max, maxRate: form.maxRate, staleSeconds: form.staleSeconds,
    resultQuality: form.resultQuality, enabled: true,
  }
  submitting.value = true
  try {
    await createQualityRule(payload)
    rulesSource.value = qualityRulesSource()
    ElMessage.success('已创建')
    showForm.value = false
    loadRules()
  } catch (e) {
    showToast(e, '保存质量规则失败')
  } finally {
    submitting.value = false
  }
}
async function remove(row: QualityRule) {
  if (!(await confirmDelete(row.name))) return
  try {
    await deleteQualityRule(row.id)
    rulesSource.value = qualityRulesSource()
    ElMessage.success('已删除')
    loadRules()
  } catch (e) {
    showToast(e, '删除质量规则失败')
  }
}

onMounted(() => { loadRules(); loadPoints() })
</script>

<style scoped>
.dq { padding: 16px; }
.stat { margin-bottom: 16px; text-align: center; }
.stat .lbl { font-size: 0.8125rem; color: var(--el-text-color-secondary); }
.stat .val { font-size: 1.625rem; font-weight: 600; margin-top: 6px; }
.stat .val.good { color: var(--text-success); }
.stat .val.bad { color: var(--text-danger); }
.stat .val.uncertain { color: var(--text-warning); }
.stat .val.old { color: var(--text-info); }
.section-card { margin-bottom: 16px; }
.top-bar { margin-bottom: 10px; display: flex; gap: 8px; }
/* S9: 1366×768 上压缩统计卡自身高度（内边距与数值字号），卡片数量与信息不变。
 * 放在所有 .stat 规则之后，避免被同名选择器覆盖。 */
@media (max-height: 800px) {
  .stat { margin-bottom: 10px; }
  .stat .val { font-size: 1.375rem; margin-top: 2px; }
  .stat :deep(.el-card__body) { padding: 10px 12px; }
}
</style>