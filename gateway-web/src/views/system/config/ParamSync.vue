<template>
  <div class="ps">
    <!-- B5：内嵌到「运行参数」时由宿主页的折叠标题说明用途，子组件不再重复画标题 -->
    <SectionHeader v-if="!embedded" title="参数同步" icon="CopyDocument" />

    <el-row :gutter="16">
      <el-col :span="8">
        <el-card shadow="never" class="section-card">
          <template #header><strong>源总线</strong></template>
          <el-radio-group v-model="sourceId" class="src-list">
            <el-radio v-for="s in sources" :key="s.id" :value="s.id" class="src-item">
              <div class="sname">{{ s.name }}</div>
              <div class="smeta">超时 {{ s.timeoutMs }}ms 重试 {{ s.retryCount }}次</div>
            </el-radio>
          </el-radio-group>
        </el-card>
      </el-col>
      <el-col :span="8">
        <el-card shadow="never" class="section-card">
          <template #header><strong>同步字段</strong></template>
          <el-checkbox-group v-model="fields" class="field-list">
            <el-checkbox v-for="f in SYNC_FIELDS" :key="f.key" :value="f.key">{{ f.label }}</el-checkbox>
          </el-checkbox-group>
          <el-divider />
          <div class="actions">
            <el-button @click="selectAll">全选</el-button>
            <el-button @click="fields = []">全不选</el-button>
          </div>
        </el-card>
      </el-col>
      <el-col :span="8">
        <el-card shadow="never" class="section-card">
          <template #header><strong>目标总线（可多选）</strong></template>
          <div class="target-list">
            <el-checkbox v-for="t in targets" :key="t.id" v-model="t.selected" class="t-item">
              <span>{{ t.name }}</span>
              <span class="cur">当前超时: {{ t.timeoutMs }}ms</span>
            </el-checkbox>
          </div>
          <el-divider />
          <el-button type="primary" :disabled="!canSync" @click="sync" :icon="Promotion" :loading="syncing">开始同步</el-button>
        </el-card>
      </el-col>
    </el-row>

    <el-card v-if="result" shadow="never" class="section-card">
      <el-alert :title="`同步完成，已将 ${result.fields.length} 个字段同步到 ${result.updated} 条总线`" type="success" show-icon :closable="false" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElMessage } from 'element-plus'
import { Promotion } from '@element-plus/icons-vue'
import { getConfig, postConfig } from '@/api/config'
import type { BusConfig } from '@/api/types'

/** embedded：作为「运行参数」页内区块使用时为 true，隐藏自身区块标题 */
defineProps<{ embedded?: boolean }>()

const SYNC_FIELDS = [
  { key: 'timeout_ms', label: '超时时间 (timeout_ms)' },
  { key: 'retry_count', label: '重试次数 (retry_count)' },
  { key: 'retry_delay_ms', label: '重试间隔 (retry_delay_ms)' },
]

interface BusItem { id: string; name: string; timeoutMs: number; retryCount: number; retryDelayMs: number; selected: boolean }

const sources = ref<BusItem[]>([])
const sourceId = ref('')
const fields = ref<string[]>(SYNC_FIELDS.map((f) => f.key))
const targets = ref<BusItem[]>([])
const result = ref<{ updated: number; fields: string[] } | null>(null)
const syncing = ref(false)

const canSync = computed(() => fields.value.length > 0 && targets.value.some((t) => t.selected))
const selectAll = () => { fields.value = SYNC_FIELDS.map((f) => f.key) }

async function load() {
  try {
    const cfg = await getConfig()
    const list: BusItem[] = (cfg.buses || []).map((b: BusConfig) => ({
      id: b.port_name, name: b.port_name, timeoutMs: b.timeout_ms, retryCount: b.retry_count ?? 3, retryDelayMs: b.retry_delay_ms, selected: false,
    }))
    sources.value = list
    targets.value = list.map((b) => ({ ...b }))
    if (list.length) sourceId.value = list[0]!.id
  } catch (e) { showToast(e, '加载失败') }
}

async function sync() {
  const src = sources.value.find((s) => s.id === sourceId.value)
  if (!src) return
  const ids = targets.value.filter((t) => t.selected).map((t) => t.id)
  if (!ids.length) return
  syncing.value = true
  try {
    const cfg = await getConfig()
    for (const b of cfg.buses || []) {
      if (ids.includes(b.port_name) && b.port_name !== src.id) {
        if (fields.value.includes('timeout_ms')) b.timeout_ms = src.timeoutMs
        if (fields.value.includes('retry_count')) b.retry_count = src.retryCount
        if (fields.value.includes('retry_delay_ms')) b.retry_delay_ms = src.retryDelayMs
      }
    }
    await postConfig(cfg)
    result.value = { updated: ids.length, fields: [...fields.value] }
    ElMessage.success(`已同步到 ${ids.length} 条总线`)
    await load()
  } catch (e) { showToast(e, '同步失败') }
  finally { syncing.value = false }
}

onMounted(load)
</script>

<style scoped>
.ps { padding: 0px; }
.section-card { margin-bottom: 16px; }
.src-list { display: flex; flex-direction: column; gap: 10px; }
.src-item { padding: 10px; border: 1px solid var(--el-border-color); border-radius: var(--radius-button); width: 100%; margin-right: 0; }
.sname { font-weight: 600; }
.smeta { font-size: 0.75rem; color: var(--el-text-color-secondary); margin-top: 2px; }
.field-list { display: flex; flex-direction: column; gap: 8px; }
.actions { display: flex; gap: 8px; }
.target-list { display: flex; flex-direction: column; gap: 6px; }
.t-item { display: flex; justify-content: space-between; align-items: center; padding: 6px; border-radius: var(--radius-small); }
.t-item:hover { background: var(--el-fill-color-light); }
.cur { font-size: 0.75rem; color: var(--el-text-color-secondary); }
</style>