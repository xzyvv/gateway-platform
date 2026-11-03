<template>
  <div class="ao">
    <SectionHeader title="地址偏移" icon="Operation" />

    <el-card shadow="never" class="section-card">
      <template #header><strong>偏移设置</strong></template>
      <el-form inline>
        <el-form-item label="偏移量">
          <el-input-number v-model="offset" :min="-9999" :max="9999" :step="1" controls-position="right" />
        </el-form-item>
        <el-form-item>
          <el-button type="primary" @click="apply" :icon="Promotion">应用偏移到选中点位</el-button>
          <el-button @click="load" :icon="Refresh">重新加载</el-button>
        </el-form-item>
      </el-form>
    </el-card>

    <el-card shadow="never" class="section-card">
      <template #header>
        <div style="display:flex;justify-content:space-between;align-items:center">
          <strong>点位列表</strong>
          <span>已选 {{ selectedCount }} 个点位</span>
        </div>
      </template>
      <DataTable
        :data="points"
        :columns="offsetColumns"
        row-key="id"
        stripe
        :loading="loading"
        empty-text="暂无点位"
      />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, h } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElCheckbox, ElMessage, ElTag } from 'element-plus'
import { confirmAction } from '@/utils/confirm'
import {Promotion, Refresh} from '@element-plus/icons-vue'
import { listOffsetPointsView, applyOffsetView, type OffsetPointViewV } from '@/api/pointsOps'
import type { ColumnDef } from '@/components/DataTable.vue'

const points = ref<OffsetPointViewV[]>([])
const offset = ref(10)
const loading = ref(false)
const selectedCount = computed(() => points.value.filter((p) => p.selected).length)

function previewAddr(p: OffsetPointViewV): number {
  if (!p.selected) return p.oldAddr
  return Math.max(0, Math.min(65535, p.oldAddr + offset.value))
}

async function load() {
  loading.value = true
  try { points.value = await listOffsetPointsView() } catch (e) { showToast(e, '加载点位失败') } finally { loading.value = false }
}

async function apply() {
  const n = selectedCount.value
  if (n === 0) {
    ElMessage.warning('请先勾选要偏移的点位')
    return
  }
  const ok = await confirmAction({
    message: `将对已勾选的 ${n} 个点位应用地址偏移 +${offset.value}，原地址会被直接改写。确定继续吗？`,
    title: '批量地址偏移确认',
    confirmText: '确认偏移',
  })
  if (!ok) return
  try {
    const r = await applyOffsetView(points.value, offset.value)
    ElMessage.success(`已偏移 ${r.updated} 个点位`)
    load()
  } catch (e) { showToast(e, '偏移操作失败') }
}

const offsetColumns: ColumnDef<OffsetPointViewV>[] = [
  {
    key: 'selected',
    title: '',
    width: 50,
    // 勾选态是业务字段（row.selected），非表格多选列，故在单元格内渲染复选框
    cell: (row) =>
      h(ElCheckbox, {
        modelValue: row.selected,
        'onUpdate:modelValue': (v: unknown) => { row.selected = Boolean(v) },
      }),
  },
  { key: 'name', title: '点位名称', minWidth: 140 },
  { key: 'type', title: '类型', width: 80 },
  { key: 'oldAddr', title: '原地址', width: 100, cell: (row) => h('span', { class: 'old' }, String(row.oldAddr)) },
  {
    key: 'newAddr',
    title: '新地址（预览）',
    width: 120,
    cell: (row) =>
      h('span', { class: ['new', { changed: previewAddr(row) !== row.oldAddr }] }, String(previewAddr(row))),
  },
  {
    key: 'delta',
    title: '变化',
    width: 80,
    cell: (row) =>
      previewAddr(row) !== row.oldAddr
        ? h(
            ElTag,
            {
              size: 'small',
              effect: 'dark',
              round: true,
              type: previewAddr(row) > row.oldAddr ? 'success' : 'warning',
            },
            () => `${previewAddr(row) > row.oldAddr ? '+' : ''}${previewAddr(row) - row.oldAddr}`,
          )
        : h('span', null, '-'),
  },
]

onMounted(load)
</script>

<style scoped>
.ao { padding: 16px; }
.section-card { margin-bottom: 16px; }
.old { color: var(--el-text-color-secondary); }
.new { font-weight: 600; }
.new.changed { color: var(--el-color-primary); }
</style>
