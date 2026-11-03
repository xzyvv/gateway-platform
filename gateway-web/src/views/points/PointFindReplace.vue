<template>
  <div class="fr">
    <SectionHeader title="查找替换" icon="Search" />

    <el-card shadow="never" class="section-card">
      <el-form inline>
        <el-form-item label="查找">
          <el-input v-model="keyword" placeholder="输入关键词" style="width:200px" clearable @keyup.enter="doSearch" />
        </el-form-item>
        <el-form-item label="替换为">
          <el-input v-model="replaceVal" placeholder="替换为" style="width:200px" />
        </el-form-item>
        <el-form-item label="范围">
          <el-select v-model="scope" style="width:120px">
            <el-option label="全部" value="all" />
            <el-option label="名称" value="name" />
            <el-option label="地址" value="address" />
            <el-option label="单位" value="unit" />
          </el-select>
        </el-form-item>
        <el-form-item>
          <el-button type="primary" @click="doSearch" :icon="Search">查找</el-button>
          <el-tooltip :disabled="!!(replaceVal && selectedCount > 0)" content="需先执行查找并勾选结果，且填写替换内容" placement="bottom">
            <span>
              <el-button type="success" @click="doReplace" :disabled="!replaceVal || selectedCount === 0">替换选中</el-button>
            </span>
          </el-tooltip>
        </el-form-item>
      </el-form>
    </el-card>

    <el-card v-if="searched" shadow="never" class="section-card">
      <template #header><strong>匹配结果</strong>（{{ targets.length }} 项，已选 {{ selectedCount }}）</template>
      <DataTable
        :data="targets"
        :columns="resultColumns"
        row-key="id"
        stripe
        :loading="loading"
        empty-text="没有匹配的点位"
      />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, h } from 'vue'
import { ElCheckbox, ElMessage } from 'element-plus'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { confirmAction } from '@/utils/confirm'
import { Search } from '@element-plus/icons-vue'
import {
  findReplaceSearchView,
  findReplaceApplyView,
  type SearchResultView,
} from '@/api/pointsOps'
import type { ColumnDef } from '@/components/DataTable.vue'

const { showToast } = useErrorDisplay()

const keyword = ref('')
const replaceVal = ref('')
const scope = ref<'name' | 'address' | 'unit' | 'all'>('all')
const targets = ref<SearchResultView[]>([])
const searched = ref(false)
const loading = ref(false)

const selectedCount = computed(() => targets.value.filter((t) => t.selected).length)

// 替换字段：mock 用 addr，后端统一用 address
const fieldMap: Record<string, string> = { name: 'name', address: 'address', unit: 'unit' }

async function doSearch() {
  // 空关键词必须拦：此前直接发请求，后端按「匹配空串」处理并返回「找到 0 项匹配」，
  // 用户无法判断是自己漏填了关键词，还是确实没有匹配点位。
  const kw = keyword.value.trim()
  if (!kw) {
    ElMessage.warning('请先填写要查找的关键词')
    return
  }
  // 结果卡片先挂出来，表格才有骨架屏可显示；失败时退回搜索前的状态
  const hadResults = searched.value
  loading.value = true
  searched.value = true
  try {
    targets.value = await findReplaceSearchView(keyword.value, scope.value)
    ElMessage.info(`找到 ${targets.value.length} 项匹配`)
  } catch (e) {
    searched.value = hadResults
    showToast(e, '加载查找结果失败')
  } finally {
    loading.value = false
  }
}

async function doReplace() {
  const n = selectedCount.value
  if (n === 0) return
  // 关键词被清空后「替换选中」仍可点：替换会把所有内容清成替换文本，必须先拦
  if (!keyword.value.trim()) {
    ElMessage.warning('查找关键词不能为空，否则会清空所选点位的字段内容')
    return
  }
  const field = fieldMap[scope.value === 'all' ? 'name' : scope.value] || 'name'
  const fieldLabel = field === 'name' ? '名称' : field === 'address' ? '地址' : '单位'
  const ok = await confirmAction({
    message: `将把已选 ${n} 个点位的「${fieldLabel}」中「${keyword.value}」替换为「${replaceVal.value}」。此操作不可撤销，确定继续吗？`,
    title: '批量替换确认',
    confirmText: '确认替换',
  })
  if (!ok) return
  // 计算预览值
  targets.value.forEach((t) => {
    if (t.selected) {
      t.newValue = t.oldValue.split(keyword.value).join(replaceVal.value)
    }
  })
  try {
    const r = await findReplaceApplyView(targets.value, field, keyword.value, replaceVal.value)
    ElMessage.success(`已替换 ${r.count} 项`)
    await doSearch()
  } catch {
    ElMessage.error('替换失败，请稍后重试')
  }
}

const resultColumns: ColumnDef<SearchResultView>[] = [
  {
    key: 'selected',
    title: '',
    width: 45,
    // 勾选态是业务字段（row.selected），非表格多选列，故在单元格内渲染复选框
    cell: (row) =>
      h(ElCheckbox, {
        modelValue: row.selected,
        'onUpdate:modelValue': (v: unknown) => { row.selected = Boolean(v) },
      }),
  },
  { key: 'deviceName', title: '设备', width: 120 },
  { key: 'pointName', title: '点位', width: 120 },
  { key: 'field', title: '字段', width: 100 },
  { key: 'oldValue', title: '原值', minWidth: 140, cell: (row) => h('code', null, row.oldValue) },
  {
    key: 'newValue',
    title: '替换后',
    minWidth: 140,
    cell: (row) => (row.newValue ? h('code', { class: 'new' }, row.newValue) : h('span', { class: 'placeholder' }, '预览')),
  },
]

// 切换 scope 自动重新搜索
watch(scope, () => { if (searched.value) doSearch() })
</script>

<style scoped>
.fr { padding: 16px; }
.section-card { margin-bottom: 16px; }
code { color: var(--el-color-primary); }
code.new { color: var(--text-success); background: var(--el-color-success-light-9); padding: 2px 6px; border-radius: 3px; }
.placeholder { color: var(--el-text-color-placeholder); }
</style>
