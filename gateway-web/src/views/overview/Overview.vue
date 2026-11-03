<template>
  <div class="dashboard" :aria-busy="loading">
    <!-- 页头与全站范式对齐：标题 + 一句话现状（判断「现在是否正常」不必先读 6 张汇总卡） -->
    <header class="dash-head">
      <div class="dash-head-text">
        <h1 class="dash-title">
          运行总览
          <!-- B1：与健康报告 / 自诊断 / 网关信息同一份聚合状态，不再各自下结论 -->
          <el-tag :type="overallStatus.type" effect="dark" round size="small" class="dash-health-tag">
            {{ overallStatus.label }}
          </el-tag>
        </h1>
        <p class="dash-sub">{{ statusSummary }}</p>
      </div>
      <div class="dash-toolbar">
        <el-button size="small" @click="showLayout = true">
          <el-icon><Setting /></el-icon> 布局设置
        </el-button>
        <el-button size="small" @click="$router.push('/ops?tab=metrics-dashboard')">
          <el-icon><Odometer /></el-icon> 查看运行指标
        </el-button>
      </div>
    </header>

    <!-- 首屏加载用骨架屏占位，而不是覆盖整页的转圈蒙层 -->
    <DashboardSkeleton v-if="loading" />
    <template v-else>
    <el-row :gutter="12" class="summary-row">
      <!-- 汇总卡可点击直达明细：数字本身不解释"去哪处理"，点击是唯一自然的下一步 -->
      <el-col :xs="12" :sm="8" :md="6" :lg="4">
        <button type="button" class="sum-card sum-c1" title="查看设备列表" @click="$router.push('/devices')">
          <div class="sum-ic"><el-icon><Cpu /></el-icon></div>
          <div class="sum-body">
            <div class="sum-val" aria-live="polite" aria-atomic="true">{{ summary.onlineDevices }}<span class="sum-sub">/{{ summary.totalDevices }}</span></div>
            <div class="sum-lbl">在线设备 / 总数</div>
          </div>
          <div class="sum-ring" :style="{ background: ringBg(summary.onlineDevices, summary.totalDevices) }"></div>
        </button>
      </el-col>
      <el-col :xs="12" :sm="8" :md="6" :lg="4">
        <button type="button" class="sum-card sum-c2" title="查看点位管理" @click="$router.push('/points')">
          <div class="sum-ic"><el-icon><Connection /></el-icon></div>
          <div class="sum-body">
            <div class="sum-val" aria-live="polite" aria-atomic="true">{{ summary.totalPoints }}</div>
            <div class="sum-lbl">点位总数</div>
          </div>
        </button>
      </el-col>
      <el-col :xs="12" :sm="8" :md="6" :lg="4">
        <button type="button" class="sum-card sum-c3" title="查看总线负载" @click="$router.push('/ops?tab=bus-load')">
          <div class="sum-ic"><el-icon><Promotion /></el-icon></div>
          <div class="sum-body">
            <div class="sum-val" aria-live="polite" aria-atomic="true">{{ summary.buses }}</div>
            <div class="sum-lbl">通信总线</div>
          </div>
        </button>
      </el-col>
      <el-col :xs="12" :sm="8" :md="6" :lg="4">
        <button type="button" class="sum-card sum-c4" title="查看日志" @click="$router.push('/system?tab=logs')">
          <div class="sum-ic"><el-icon><Warning /></el-icon></div>
          <div class="sum-body">
            <div class="sum-val" aria-live="polite" aria-atomic="true">{{ summary.alerts }}</div>
            <div class="sum-lbl">今日告警</div>
          </div>
        </button>
      </el-col>
      <el-col :xs="12" :sm="8" :md="6" :lg="4">
        <button type="button" class="sum-card sum-c5" title="查看运行指标" @click="$router.push('/ops?tab=metrics-dashboard')">
          <div class="sum-ic"><el-icon><DataLine /></el-icon></div>
          <div class="sum-body">
            <div class="sum-val" aria-live="polite" aria-atomic="true">{{ formatNum(summary.collectToday) }}</div>
            <div class="sum-lbl">今日采集量</div>
          </div>
        </button>
      </el-col>
      <el-col :xs="12" :sm="8" :md="6" :lg="4">
        <button type="button" class="sum-card sum-c6" :class="{ ok: summary.gatewayOnline, bad: !summary.gatewayOnline }" title="查看系统状态" @click="$router.push('/system')">
          <div class="sum-ic"><el-icon><Monitor /></el-icon></div>
          <div class="sum-body">
            <div class="sum-val" aria-live="polite" aria-atomic="true">{{ summary.gatewayOnline ? '运行中' : '离线' }}</div>
            <div class="sum-lbl">网关状态</div>
          </div>
        </button>
      </el-col>
    </el-row>

    <!-- 异常聚合：把「哪几处不对」压成一屏，每条直达处理页 -->
    <AttentionPanel v-if="isVisible('attention')" :signals="healthSignals" />

    <!-- 版本信息降级为页脚：它不是日常判断依据，不该占一张卡片的视觉重量 -->
    <footer class="dash-footer">
      <span class="df-label">版本信息</span>
      <span>v{{ version.version }}</span>
      <span class="df-sep">·</span>
      <span>构建 {{ version.build_time }}</span>
      <span class="df-sep">·</span>
      <span class="df-mono">{{ version.git_hash }}</span>
      <span v-if="lastUpdate || dataStale" class="df-freshness">
        <span v-if="dataStale" class="stale-hint" title="网关不可达，当前展示最近一次成功获取的缓存数据">
          <el-icon><Warning /></el-icon> {{ dataStaleText }}
        </span>
        <span v-if="lastUpdate" class="last-update-hint">
          <el-icon><Clock /></el-icon> {{ lastUpdateText }}
        </span>
      </span>
    </footer>

    <!-- N7: 设备概览与组件健康状态并排。此前两张卡片纵向堆叠，1366×768 下
         需要滚动才能看到组件健康状态；并排后两者同屏，首屏纵向高度直接减半。 -->
    <el-row v-if="isVisible('devices') || isVisible('health')" :gutter="14" class="lower-row">
      <el-col v-if="isVisible('devices')" :xs="24" :lg="14">
        <DeviceSummary :device-list="deviceList" @device-click="goDevice" />
      </el-col>
      <el-col v-if="isVisible('health')" :xs="24" :lg="10">
        <HealthStatus :health-rows="healthRows" :loading="loading" />
      </el-col>
    </el-row>
    </template>

    <el-dialog v-model="showLayout" title="仪表盘布局设置" width="420px">
      <p style="font-size: 0.8125rem;color:var(--text-secondary);margin:0 0 12px">控制各模块的显示，设置自动保存到本地。</p>
      <DataTable
        :data="layoutSections"
        :columns="layoutColumns"
        row-key="key"
        stripe
        :loading="loading"
        empty-text="暂无可配置的布局模块"
      />
      <template #footer>
        <el-button @click="layoutReset()">恢复默认</el-button>
        <el-button type="primary" @click="showLayout = false">完成</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { h, computed } from 'vue'
import { ElSwitch } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import type { DashboardSection } from '@/composables/dashboard/useDashboardLayout'
import { useDashboardData } from '@/composables/dashboard/useDashboardData'
import AttentionPanel from '@/components/overview/AttentionPanel.vue'
import DashboardSkeleton from '@/components/overview/DashboardSkeleton.vue'
import DeviceSummary from '@/components/overview/DeviceSummary.vue'
import HealthStatus from './HealthStatus.vue'

const {
  showLayout, layoutSections, layoutToggle, layoutReset, isVisible, goDevice,
  summary, ringBg, formatNum,
  version, healthRows, deviceList, loading,
  overallStatus,
  healthSignals,
  lastUpdate, lastUpdateText,
  dataStale, dataStaleText,
} = useDashboardData()

/**
 * 页头现状摘要：先给结论（正常 / 有告警 / 不可达），再给依据。
 * 放在页头而非卡片里，是因为它要回答「要不要现在处理」，值不值得点开任何一张卡。
 */
const statusSummary = computed(() => {
  if (!summary.gatewayOnline) return '网关当前不可达，以下为最近一次成功获取的数据'
  if (dataStale.value) return '数据已过期，正在等待网关重新同步'
  // 0 设备时「0/0 台设备在线」读起来像一切正常，实际是还没开始接入
  if (summary.totalDevices === 0) return '尚未接入任何设备，请先完成第一台设备的接入'
  const base = `${summary.onlineDevices}/${summary.totalDevices} 台设备在线`
  return summary.alerts > 0 ? `${base} · ${summary.alerts} 条今日告警待处理` : `${base} · 今日无告警`
})

// 布局设置表列定义：模块名 + 显示开关（沿用 v-model + change 的双回调语义）
const layoutColumns: ColumnDef<DashboardSection>[] = [
  { key: 'label', title: '模块' },
  {
    key: 'visible',
    title: '显示',
    width: 100,
    cell: (row) =>
      h(ElSwitch, {
        modelValue: row.visible,
        'onUpdate:modelValue': (v: unknown) => { row.visible = Boolean(v) },
        onChange: () => layoutToggle(row.key),
      }),
  },
]
</script>

<style scoped src="./Overview.css"></style>