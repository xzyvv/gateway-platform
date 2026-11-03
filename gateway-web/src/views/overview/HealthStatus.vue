<template>
  <el-card shadow="never" class="section-card">
    <template #header>
      <SectionHeader title="组件健康状态" icon="Connection">
        <template #extra>
          <el-tag v-if="status" :type="status.type" effect="dark" round size="small">{{ status.label }}</el-tag>
        </template>
      </SectionHeader>
    </template>
    <DataTable
      :data="healthRows"
      :columns="healthColumns"
      :loading="loading"
      row-key="name"
      empty-text="暂无组件状态数据"
    />
  </el-card>
</template>

<script setup lang="ts">
import { h } from 'vue'
import { ElIcon, ElTag } from 'element-plus'
import { Cpu } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import type { HealthComponent } from '@/api/types'
import type { OverallHealthStatus } from '@/composables/system/useHealthReport'
import { healthComponentLabel, healthStatusInfo } from '@/utils/healthStatus'

// status 可选：给出综合状态（正常/降级/严重），与 useHealthReport 的 overallStatus 同一文案
defineProps<{ healthRows: HealthComponent[]; loading?: boolean; status?: OverallHealthStatus }>()

// 组件名 / 状态名 / 徽章色统一走 utils/healthStatus：
// 那里登记的是后端 /health/ready 的真实取值（组件 process/mqtt/queue/ota，
// 状态 ok/down/backpressure/degraded），前端不再另立一套映射。

// 表格统一走 DataTable：状态列由模板 #default 改成 cell 渲染，列宽/列顺序与迁移前一致
const healthColumns: ColumnDef<HealthComponent>[] = [
  {
    key: 'name',
    title: '组件',
    width: 200,
    cell: (row) =>
      h('div', { class: 'health-name' }, [
        h(ElIcon, { class: 'health-icon' }, () => h(Cpu)),
        h('span', healthComponentLabel(row.name)),
      ]),
  },
  {
    key: 'status',
    title: '状态',
    cell: (row) => {
      const info = healthStatusInfo(row.status)
      return h(ElTag, { type: info.tag, effect: 'dark', round: true }, () => info.label)
    },
  },
]
</script>

<!--
  组件样式必须自带，不能留在使用方页面的 scoped css 里：
  本表格现在同时用在运行总览页与「系统 → 网关信息」页，
  而表格单元格是 render 函数里 h() 出来的元素，拿不到使用方页面的 scope id，
  样式写在页面里等于不生效。
-->
<style scoped>
.health-name {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-weight: 500;
}
.health-icon {
  width: 20px;
  height: 20px;
  border-radius: 5px;
  background: rgba(var(--color-primary-rgb), 0.1);
  color: var(--color-primary);
  font-size: 0.75rem;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: background-color, transform 0.2s;
}
.health-name:hover .health-icon {
  background: rgba(var(--color-primary-rgb), 0.18);
  transform: scale(1.1);
}
</style>
