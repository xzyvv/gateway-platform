<template>
  <div class="realtime">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="点位实时值" icon="DataLine">
          <template #extra>
            <el-tag :type="connected ? 'success' : (reconnectAttempts > 0 ? 'warning' : 'danger')" effect="dark" round size="small" class="conn-tag">
              <span class="dot" :class="{ active: connected, reconnecting: reconnectAttempts > 0 && !connected }"></span>
              {{ connected ? '已连接' : (reconnectAttempts > 0 ? `重连中 (${reconnectAttempts})` : '未连接') }}
            </el-tag>
            <!-- 首帧到达前不能显示「0 个点位」：那会被读成「一个点位都没有」，
                 实际只是 WebSocket 刚建立、采集帧还没推过来 -->
            <el-tag type="info" effect="plain" round size="small" class="count-badge">
              {{ pointList.length ? `${pointList.length} 个点位` : (connected ? '等待首帧数据…' : '连接中…') }}
            </el-tag>
          </template>
          <el-button
            size="small"
            :type="paused ? 'warning' : 'info'"
            plain
            class="pause-btn"
            @click="togglePause"
          >
            <el-icon><VideoPlay v-if="paused" /><VideoPause v-else /></el-icon>
            {{ paused ? '继续刷新' : '暂停刷新' }}
          </el-button>
          <el-tag v-if="paused" type="warning" effect="dark" round size="small" class="paused-tag">
            已冻结
          </el-tag>
          <el-checkbox v-model="onlyFavorites" class="fav-filter">
            <el-icon><StarFilled v-if="onlyFavorites" style="color: var(--color-warning)" /><Star v-else /></el-icon>
            仅看收藏
          </el-checkbox>
          <el-button size="small" type="success" plain @click="exportCsv" :disabled="!displayPoints.length">
            <el-icon><Download /></el-icon> 导出 CSV
          </el-button>
        </SectionHeader>
      </template>
      <!-- 陈旧数据常驻提示：WS 断开但表里还有值时，表格仍会显示上一次推送的数字。
           现场最危险的误读就是把「5 分钟前的值」当成实时值，所以这里必须显式告知。 -->
      <div
        v-if="staleWhileDisconnected"
        class="stale-banner"
        role="status"
        aria-live="polite"
      >
        <el-icon class="stale-ico"><WarningFilled /></el-icon>
        <span class="stale-text">
          实时连接已断开，下方为最后一次推送的值（最后更新 <FreshnessBadge :timestamp="lastUpdateTs" />），
          <strong>不是实时值</strong>。
        </span>
        <el-button size="small" type="warning" plain @click="reconnectNow">
          <el-icon><Refresh /></el-icon> 立即重连
        </el-button>
      </div>
      <!-- A4: 表格统一走 DataTable，点位表上千行时自动切虚拟滚动 -->
      <DataTable
        :data="tableRows"
        :columns="realtimeColumns"
        :row-key="realtimeRowKey"
        :row-class-name="realtimeRowClass"
        :height="tableHeight"
        :max-height="tableHeight"
        :estimated-row-height="44"
        :loading="loading"
      >
        <template #empty>
          <!-- N4: 空态按原因分区——筛选空 / 未连接 / 等待首帧，各自给出可执行的下一步 -->
          <empty-state
            :description="emptyText"
            :image-size="80"
            :action-text="emptyActionText"
            :action-type="emptyKind === 'filtered' ? 'default' : 'primary'"
            @action="onEmptyAction"
          />
        </template>
      </DataTable>
    </el-card>
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="数据趋势" icon="TrendCharts">
          <template #extra>
            <el-tag type="info" effect="plain" round size="small" class="count-tag">
              {{ chartTimeSpan }}
            </el-tag>
          </template>
        </SectionHeader>
      </template>
      <div class="chart-wrap" :style="{ height: `${chartHeight}px` }">
        <div ref="chartRef" class="chart" data-chart-label="实时数据趋势图" :style="{ height: `${chartHeight}px` }"></div>
        <div v-if="pointList.length === 0" class="chart-empty">
          <el-icon :size="40"><TrendCharts /></el-icon>
          <p>{{ emptyText }}</p>
          <el-button v-if="emptyActionRoute" type="primary" link @click="onEmptyAction">{{ emptyActionText }}</el-button>
        </div>
        <!-- BUG-K：渲染异常必须可见，否则用户会把「画布空白」误判成「没有数据」 -->
        <div v-else-if="chartError" class="chart-empty">
          <el-icon :size="40"><WarningFilled /></el-icon>
          <p>趋势图渲染失败：{{ chartError }}</p>
        </div>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { h, computed, watch } from 'vue'
import { useRouter } from 'vue-router'
import { storeToRefs } from 'pinia'
import { ElButton, ElIcon, ElTag, ElTooltip } from 'element-plus'
import { StarFilled, Star, Refresh, Top, Bottom, WarningFilled } from '@element-plus/icons-vue'
import FreshnessBadge from '@/components/FreshnessBadge.vue'
import { useWebsocketStore } from '@/stores/websocket'
import type { ColumnDef } from '@/components/DataTable.vue'
import type { DataPoint } from '@/api/types'
import { useRealtime } from '@/composables/data/useRealtime'
import { useQueryState } from '@/composables/core/useQueryState'

type RealtimePoint = DataPoint & {
  _flash?: boolean
  _surge?: 'up' | 'down'
  _surgePct?: number
  _highlight?: boolean
}

/** DataTable 行键：点位 ID 全局唯一 */
const realtimeRowKey = (row: RealtimePoint) => row.point_id

const {
  pointList, connected, reconnectAttempts, onlyFavorites, paused,
  displayPoints, emptyText, emptyKind, emptyActionText, emptyActionRoute, tableHeight, chartHeight, chartRef, chartTimeSpan, chartError,
  isFavorite, toggleFavorite,
  qualityClass, qualityTagType, qualityLabel, typeLabel,
  formatValue, formatTime,
  reconnectNow, exportCsv, togglePause,
} = useRealtime()

const router = useRouter()

/** N4: 空态主行动——取消筛选 / 重连 / 跳到解决该问题的页面 */
function onEmptyAction() {
  if (emptyKind.value === 'filtered') onlyFavorites.value = false
  else if (emptyActionRoute.value) router.push(emptyActionRoute.value)
  else reconnectNow()
}

// 模板里的 ref="chartRef" 是按名字绑定 setup 变量的，vue-tsc 的 noUnusedLocals 看不到这层引用
void chartRef

/**
 * 表格加载态：WebSocket 首次建连期间显示骨架屏，避免首屏纯空白无反馈。
 *
 * 数据是 WS 推送的（没有 fetch 可挂 try/catch），故用连接状态派生：
 *   - status='connecting' 且 reconnectAttempts=0 → 正在首次建连；
 *   - 一旦进入重连（reconnectAttempts>0），必须让空态露出来，
 *     否则空态里的「立即重连」按钮会被骨架屏顶掉（功能回归）。
 * 连上后由 emptyText 提示"已连接，等待首帧采集数据…"，不再显示骨架屏。
 */
const wsStore = useWebsocketStore()
const { status: wsStatus } = storeToRefs(wsStore)
const loading = computed(() => wsStatus.value === 'connecting' && reconnectAttempts.value === 0)

// 仅看收藏开关与 URL 同步：切 tab 返回后还原（?fav=1）
const qsFav = useQueryState('fav')
if (qsFav.value === '1') onlyFavorites.value = true
watch(onlyFavorites, (v) => { qsFav.value = v ? '1' : '' })

// 深链跳入（?highlight=<point_id>）：目标点位置顶 + 行高亮
const qsHighlight = useQueryState('highlight')

// 最近一次推送的时间戳：断开连接时用它告诉用户数据"停在多久之前"
const lastUpdateTs = computed(() => {
  let max = 0
  for (const p of pointList.value) {
    const ts = Number(p.timestamp)
    if (Number.isFinite(ts) && ts > max) max = ts
  }
  return max
})

// 已断开但表里仍有数据 → 表格显示的是旧值，必须显式标注
const staleWhileDisconnected = computed(() => !connected.value && pointList.value.length > 0)

const tableRows = computed<RealtimePoint[]>(() => {
  const id = qsHighlight.value
  const rows = displayPoints.value as RealtimePoint[]
  if (!id || !rows.length) return rows
  const idx = rows.findIndex((p) => p.point_id === id)
  if (idx < 0) return rows
  const hit: RealtimePoint = { ...rows[idx]!, _highlight: true }
  return [hit, ...rows.filter((_, i) => i !== idx)]
})

/** 行级异常标记：bad 红底 / stale、uncertain 黄底；highlight 行闪烁定位 */
function realtimeRowClass(row: RealtimePoint): string {
  const cls: string[] = []
  if (row.quality === 'bad') cls.push('row-quality-bad')
  else if (row.quality === 'stale' || row.quality === 'uncertain') cls.push('row-quality-warn')
  if (row._highlight) cls.push('row-highlight')
  return cls.join(' ')
}

// A4: DataTable 列定义。cell 直接拿到整行，复刻原模板渲染。
// 列宽说明：虚拟滚动引擎只接受固定像素宽（不支持弹性 min-width），所以这里必须
// 控制总宽。原 8 列合计 1028px，在 1280×720 笔记本（内容区约 977px）会横向滚动；
// 把点位 ID 与更新时间两列各收紧 30px 后合计 968px，1280 起不再溢出。
// 收窄依赖 DataTable 默认的 ellipsis + tooltip，长文本不会丢。
const realtimeColumns = computed<ColumnDef<RealtimePoint>[]>(() => [
  {
    key: 'fav',
    title: '★',
    width: 48,
    align: 'center',
    fixed: 'left',
    cell: (row) =>
      h(
        ElButton,
        {
          link: true,
          class: ['fav-btn', isFavorite(row.point_id) ? 'active' : ''],
          onClick: (e: Event) => {
            e.stopPropagation()
            toggleFavorite(row.point_id)
          },
          title: isFavorite(row.point_id) ? '取消收藏' : '收藏',
        },
        () =>
          h(ElIcon, { size: 16 }, () =>
            isFavorite(row.point_id) ? h(StarFilled) : h(Star)
          ),
      ),
  },
  {
    key: 'point_id',
    title: '点位 ID',
    width: 170,
    cell: (row) => h('span', { class: 'point-id' }, row.point_id),
  },
  {
    key: 'device_id',
    title: '设备',
    width: 140,
    cell: (row) => h('span', row.device_id),
  },
  {
    key: 'value',
    title: '当前值',
    width: 120,
    cell: (row) =>
      h('span', { class: 'value-wrap' }, [
        h(
          ElTooltip,
          {
            // A4 修复: quality 可能 undefined（运行时点位对象可能没 quality 字段），加 || '' 兜底
            content: qualityLabel[row.quality || ''] || row.quality || '',
            placement: 'top',
            showAfter: 300,
          },
          () =>
            h(
              'span',
              {
                key: `${row.point_id}-${row.value}`,
                class: ['value-cell', qualityClass(row.quality || ''), row._flash ? 'is-flash' : ''],
              },
              formatValue(row.value),
            ),
        ),
        // 突变标记：单帧变化 ≥50% 时显示方向箭头，tooltip 给出幅度
        row._surge
          ? h(
              ElTooltip,
              {
                content: `较上次变化 ${row._surgePct && row._surgePct > 0 ? '+' : ''}${row._surgePct ?? ''}%，疑似突变`,
                placement: 'top',
                showAfter: 200,
              },
              () =>
                h(
                  ElIcon,
                  { size: 14, class: ['surge-icon', row._surge === 'up' ? 'surge-up' : 'surge-down'] },
                  () => (row._surge === 'up' ? h(Top) : h(Bottom)),
                ),
            )
          : null,
      ]),
  },
  {
    key: 'raw_value',
    title: '原始值',
    width: 120,
    cell: (row) => h('span', { class: 'raw-value' }, formatValue(row.raw_value)),
  },
  {
    key: 'raw_type',
    title: '类型',
    width: 110,
    cell: (row) =>
      h(ElTag, { size: 'small', type: 'info', effect: 'dark', round: true }, () =>
        (typeLabel[row.raw_type] || row.raw_type),
      ),
  },
  {
    key: 'quality',
    title: '质量',
    width: 90,
    cell: (row) =>
      h(
        ElTag,
        { type: qualityTagType(row.quality), effect: 'dark', size: 'small', round: true },
        () => (qualityLabel[row.quality] || row.quality),
      ),
  },
  {
    key: 'timestamp',
    title: '更新时间',
    width: 170,
    cell: (row) =>
      h('div', { class: 'time-cell' }, [
        h('span', formatTime(row.timestamp)),
        h(FreshnessBadge, { timestamp: row.timestamp }),
      ]),
  },
])
</script>

<style scoped src="./RealtimeMonitor.css"></style>
