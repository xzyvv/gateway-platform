<!-- 日志归档：列出网关日志目录下真实的按日滚动文件，支持查看尾部与下载 -->
<template>
  <div class="la">
    <SectionHeader title="日志归档" icon="Folder">
      <template #extra>
        <el-button :icon="Refresh" :loading="loading" @click="load">刷新</el-button>
        <el-button :icon="Download" @click="downloadCurrent">下载当期日志</el-button>
      </template>
    </SectionHeader>

    <el-card shadow="never" class="section-card">
      <el-alert
        type="info"
        :closable="false"
        show-icon
        title="日志按天滚动写入网关日志目录（GATEWAY_LOG_DIR，默认 /var/log/gateway），文件名为 gateway.log.YYYY-MM-DD"
        description="列表由后端扫描真实日志目录得出；保留天数取决于磁盘轮转策略。点「查看」读取该日日志尾部，点「下载」导出完整文件。"
      />
      <DataTable
        :data="archives"
        :columns="archiveColumns"
        row-key="date"
        stripe
        empty-text="日志目录下暂无归档文件（网关可能刚启动，或日志被定向到 stderr/外部收集器）"
      />
    </el-card>

    <el-drawer v-model="drawer" :title="`${viewDate} 日志尾部`" size="65%">
      <div v-if="loadingLines" class="loading-line">正在读取日志…</div>
      <template v-else>
        <el-alert
          v-if="viewError"
          type="warning"
          :closable="false"
          show-icon
          :title="viewError"
        />
        <EmptyState
          v-else-if="!logLines.length"
          description="该日没有日志内容：文件可能为空，或已被磁盘轮转清理"
          :image-size="80"
        />
        <div v-else class="log-list">
          <div v-for="(l, i) in logLines" :key="i" class="log-line">
            <span class="time">{{ shortTime(l.time) }}</span>
            <el-tag v-if="l.level" :type="levelType(l.level)" size="small" class="level">{{ l.level }}</el-tag>
            <span class="target">{{ l.target }}</span>
            <span class="msg">{{ l.message }}</span>
          </div>
        </div>
      </template>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
/**
 * 日志归档
 *
 * 数据源为后端真实日志目录扫描（`/api/diagnostics/logs/archives`）与日志下载
 * （`/api/diagnostics/logs/download?date=`）。旧实现把归档元数据存在 localStorage，
 * 首次访问时凭空生成 7 条「报文数为 0」的记录，现场排障会把假列表当成网关无日志。
 */
import { ref, onMounted, h } from 'vue'
import { ElButton, ElTag, ElMessage } from 'element-plus'
import { Refresh, Download } from '@element-plus/icons-vue'
import DataTable, { type ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import EmptyState from '@/components/EmptyState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import {
  downloadArchive,
  listArchives,
  viewArchive,
  type ArchiveLogLine,
  type LogArchiveItem,
} from '@/api/logArchive'

const loading = ref(false)
const archives = ref<LogArchiveItem[]>([])

const drawer = ref(false)
const viewDate = ref('')
const loadingLines = ref(false)
const viewError = ref('')
const logLines = ref<ArchiveLogLine[]>([])

function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 KB'
  const kb = bytes / 1024
  return kb >= 1024 ? `${(kb / 1024).toFixed(1)} MB` : `${kb.toFixed(0)} KB`
}

/** RFC3339 时间戳只保留时分秒，列表里更好扫读 */
function shortTime(ts: string): string {
  const m = ts.match(/T(\d{2}:\d{2}:\d{2})/)
  return m?.[1] ?? ts
}

function levelType(level: string): 'success' | 'warning' | 'danger' | 'info' {
  if (level === 'ERROR' || level === 'WARN') return level === 'ERROR' ? 'danger' : 'warning'
  if (level === 'DEBUG' || level === 'TRACE') return 'info'
  return 'success'
}

const archiveColumns: ColumnDef<LogArchiveItem>[] = [
  { key: 'date', title: '日期', width: 130 },
  {
    key: 'size_bytes',
    title: '大小',
    width: 110,
    cell: (row) => formatSize(row.size_bytes),
  },
  {
    key: 'current',
    title: '类型',
    width: 100,
    cell: (row) =>
      h(ElTag, { type: row.current ? 'success' : 'info', size: 'small' }, () =>
        row.current ? '当期' : '历史',
      ),
  },
  actionColumn<LogArchiveItem>({
    width: 160,
    cell: (row) =>
      h('div', { class: 'row-actions' }, [
        h(
          ElButton,
          { size: 'small', link: true, type: 'primary', onClick: () => view(row.date) },
          () => '查看',
        ),
        h(
          ElButton,
          { size: 'small', link: true, type: 'primary', onClick: () => download(row.date) },
          () => '下载',
        ),
      ]),
  }),
]

async function load() {
  loading.value = true
  try {
    archives.value = await listArchives()
  } catch {
    archives.value = []
  } finally {
    loading.value = false
  }
}

async function view(date: string) {
  viewDate.value = date
  viewError.value = ''
  logLines.value = []
  drawer.value = true
  loadingLines.value = true
  try {
    const r = await viewArchive(date)
    logLines.value = r.lines
  } catch (e) {
    // 取样失败时明确告知原因：不能让用户把空表当成「当天确实没有日志」
    viewError.value = e instanceof Error && e.message ? e.message : '获取日志失败'
  } finally {
    loadingLines.value = false
  }
}

async function download(date: string) {
  try {
    await downloadArchive(date)
    ElMessage.success(`已下载 ${date} 的日志`)
  } catch (e) {
    ElMessage.error(e instanceof Error && e.message ? e.message : '下载日志失败')
  }
}

async function downloadCurrent() {
  try {
    await downloadArchive()
    ElMessage.success('已下载当期日志')
  } catch (e) {
    ElMessage.error(e instanceof Error && e.message ? e.message : '下载日志失败')
  }
}

onMounted(load)
</script>

<style scoped>
.la {
  padding: 16px;
}
.section-card {
  margin-bottom: 16px;
}
.row-actions {
  display: flex;
  gap: 8px;
}
.loading-line {
  color: var(--text-secondary);
  font-size: 0.875rem;
  padding: 12px 0;
}
.log-list {
  font-family: monospace;
  font-size: 0.8125rem;
}
.log-line {
  display: flex;
  gap: 10px;
  align-items: baseline;
  padding: 4px 0;
  border-bottom: 1px solid var(--el-border-color-lighter);
}
.time {
  color: var(--el-text-color-secondary);
  min-width: 72px;
  flex: 0 0 auto;
}
.level {
  flex: 0 0 auto;
}
.target {
  color: var(--el-text-color-secondary);
  flex: 0 0 auto;
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.msg {
  color: var(--el-color-primary);
  word-break: break-all;
}
</style>
