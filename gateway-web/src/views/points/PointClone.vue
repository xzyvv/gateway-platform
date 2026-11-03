<template>
  <div class="pc2">
    <SectionHeader title="点位克隆" icon="Collection" />

    <el-row :gutter="16">
      <el-col :span="6">
        <el-card shadow="never" class="section-card">
          <template #header><strong>源设备（模板）</strong></template>
          <el-radio-group v-model="sourceId" class="source-list" @change="onSourceChange">
            <el-radio v-for="d in sources" :key="d.id" :value="d.id" class="source-item">
              <div class="sname">{{ d.name }}</div>
              <div class="smeta">{{ d.pointCount }} 个点位</div>
            </el-radio>
          </el-radio-group>
        </el-card>
      </el-col>
      <el-col :span="10">
        <el-card shadow="never" class="section-card">
          <template #header><strong>源点位预览（{{ currentSource?.pointCount || 0 }} 个）</strong></template>
          <DataTable
            :data="sourcePoints"
            :columns="sourcePointColumns"
            row-key="id"
            stripe
            :loading="loading"
            empty-text="选择左侧源设备查看点位"
          />
        </el-card>
      </el-col>
      <el-col :span="8">
        <el-card shadow="never" class="section-card">
          <template #header><strong>目标设备（可多选）</strong></template>
          <div class="target-list">
            <el-checkbox v-for="t in targets" :key="t.deviceId" v-model="t.selected" class="target-item">
              {{ t.deviceName }} ({{ t.pointCount }} 点)
            </el-checkbox>
          </div>
          <el-divider />
          <el-form label-width="110px" size="small">
            <el-form-item label="点位名称后缀">
              <el-input v-model="nameSuffix" placeholder="如 _#2，留空则不变" />
            </el-form-item>
            <el-form-item>
              <el-tooltip :disabled="canClone" content="请先选择源设备与目标设备" placement="top">
                <span>
                  <el-button type="primary" :disabled="!canClone" @click="doClone"><el-icon><Promotion /></el-icon> 开始克隆</el-button>
                </span>
              </el-tooltip>
            </el-form-item>
          </el-form>
        </el-card>
      </el-col>
    </el-row>

    <el-card v-if="result" shadow="never" class="section-card">
      <el-alert :title="`克隆完成，共 ${result.total} 个点位到 ${result.targets.length} 台设备`" type="success" show-icon :closable="false" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { ElMessage } from 'element-plus'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { confirmAction } from '@/utils/confirm'
import {Promotion} from '@element-plus/icons-vue'
import {
  listCloneSourcesView,
  listCloneTargetsView,
  cloneApplyView,
  type CloneSourceView,
  type CloneTargetView,
} from '@/api/pointsOps'
import { listPoints } from '@/api/points'
import type { ColumnDef } from '@/components/DataTable.vue'

/** 源设备点位预览行（点位级） */
type SourcePointRow = { id: string; name: string; address_num: number; data_type: string }

const { showToast } = useErrorDisplay()

const sources = ref<CloneSourceView[]>([])
const targets = ref<CloneTargetView[]>([])
const sourceId = ref('')
const sourcePoints = ref<SourcePointRow[]>([])
const nameSuffix = ref('')
const result = ref<{ total: number; targets: string[] } | null>(null)
/** 源点位预览表的加载态：首次加载与切换源设备时都置位 */
const loading = ref(false)

const currentSource = computed(() => sources.value.find((d) => d.id === sourceId.value))
const canClone = computed(() => !!sourceId.value && targets.value.some((t) => t.selected))

async function loadSources() {
  loading.value = true
  try {
    sources.value = await listCloneSourcesView()
    if (sources.value.length > 0 && !sourceId.value) {
      sourceId.value = sources.value[0]?.id || ''
    }
    await onSourceChange()
  } catch (e) {
    showToast(e, '加载源设备失败')
  } finally {
    loading.value = false
  }
}

async function onSourceChange() {
  if (!sourceId.value) {
    sourcePoints.value = []
    return
  }
  loading.value = true
  try {
    const all = await listPoints(sourceId.value)
    sourcePoints.value = all.map((p) => ({
      id: p.id,
      name: p.name || p.id,
      address_num: p.address_num || 0,
      data_type: p.data_type || 'f32',
    }))
  } catch (e) {
    sourcePoints.value = []
    showToast(e, '加载点位失败')
  } finally {
    loading.value = false
  }
}

async function doClone() {
  const src = currentSource.value
  const selectedTargets = targets.value.filter((t) => t.selected)
  if (!src || selectedTargets.length === 0) return
  const ok = await confirmAction({
    message: `将从设备「${src.name}」克隆 ${src.pointCount} 个点位到 ${selectedTargets.length} 台目标设备（${selectedTargets.map((t) => t.deviceName).join('、')}），共约 ${src.pointCount * selectedTargets.length} 个点位将被创建。确定继续吗？`,
    title: '点位克隆确认',
    confirmText: '确认克隆',
  })
  if (!ok) return
  try {
    const r = await cloneApplyView(sourceId.value, targets.value, nameSuffix.value)
    result.value = r
    ElMessage.success(`已克隆 ${r.total} 个点位`)
    // 重新加载目标和源（点位数会变）
    await loadTargets()
    await loadSources()
  } catch {
    ElMessage.error('克隆失败，请稍后重试')
  }
}

async function loadTargets() {
  try {
    targets.value = await listCloneTargetsView()
  } catch (e) {
    showToast(e, '加载目标设备失败')
  }
}

const sourcePointColumns: ColumnDef<SourcePointRow>[] = [
  { key: 'name', title: '名称' },
  { key: 'address_num', title: '地址', width: 80 },
  { key: 'data_type', title: '类型', width: 80 },
]

onMounted(async () => {
  await Promise.all([loadSources(), loadTargets()])
})
</script>

<style scoped>
.pc2 { padding: 0px; }
.section-card { margin-bottom: 16px; }
.source-list { display: flex; flex-direction: column; gap: 8px; }
.source-item { padding: 8px; border: 1px solid var(--el-border-color); border-radius: var(--radius-button); width: 100%; margin-right: 0; }
.sname { font-weight: 600; }
.smeta { font-size: 0.75rem; color: var(--el-text-color-secondary); }
.target-list { display: flex; flex-direction: column; gap: 6px; max-height: 260px; overflow-y: auto; }
</style>
