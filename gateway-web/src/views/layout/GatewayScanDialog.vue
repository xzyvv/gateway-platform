<template>
  <!-- LAN 网关扫描结果弹窗 -->
  <el-dialog
    :model-value="modelValue"
    @update:model-value="(v: boolean) => $emit('update:modelValue', v)"
    title="局域网网关扫描结果"
    width="560px"
    :close-on-click-modal="false"
  >
    <div v-if="scanning" class="scan-loading">
      <el-icon class="is-loading"><Loading /></el-icon>
      <span>正在扫描局域网，请稍候...</span>
    </div>
    <div v-else-if="results.length === 0" class="scan-empty">
      <el-icon><WarningFilled /></el-icon>
      <span>未发现网关设备。请确认网关已启动且与本机在同一网段。</span>
    </div>
    <DataTable
      v-else
      :data="results"
      :columns="scanColumns"
      :row-key="scanRowKey"
      stripe
    />
    <template #footer>
      <el-button @click="$emit('update:modelValue', false)">关闭</el-button>
      <el-button type="primary" :loading="scanning" :disabled="scanning" @click="$emit('rescan')">重新扫描</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
/**
 * 局域网网关扫描结果弹窗
 *
 * 内部不含 cron/event 副作用，仅做展示：
 *   - scanning=true：渲染加载态
 *   - results.length===0：渲染空态
 *   - 否则：渲染 el-table + 每行"新增"按钮
 *
 * 通过 v-model:modelValue 控制显示；事件：
 *   - add(row)：请求父组件加入配置列表
 *   - rescan：请求父组件重新触发扫描
 *   - update:modelValue：弹窗关闭（v-model 双向）
 */
import { h } from 'vue'
import { Loading, WarningFilled } from '@element-plus/icons-vue'
import { ElButton } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import type { DiscoveredGateway } from '@/api'

const props = defineProps<{
  modelValue: boolean
  scanning: boolean
  results: DiscoveredGateway[]
  /** 新增按钮禁用（如正在切换网关） */
  addDisabled?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', v: boolean): void
  (e: 'add', gw: DiscoveredGateway): void
  (e: 'rescan'): void
}>()

/** 扫描结果行唯一键（原表无 row-key，用 ip:port 组合保证唯一） */
function scanRowKey(row: DiscoveredGateway): string {
  return `${row.ip}:${row.port}`
}

const scanColumns: ColumnDef<DiscoveredGateway>[] = [
  { key: 'name', title: '名称', minWidth: 140, ellipsis: true },
  { key: 'ip', title: 'IP 地址', width: 130 },
  { key: 'port', title: '端口', width: 70 },
  { key: 'latency_ms', title: '延迟', width: 80, cell: (row) => `${row.latency_ms}ms` },
  actionColumn<DiscoveredGateway>({
    key: 'action',
    width: 90,
    fixed: 'right',
    cell: (row) =>
      h(
        ElButton,
        {
          size: 'small',
          type: 'primary',
          text: true,
          // 扫描中列表正在刷新，此时新增拿到的是过期结果，先禁用
          disabled: props.addDisabled || props.scanning,
          onClick: () => emit('add', row),
        },
        () => '新增',
      ),
  }),
]
</script>

<style src="./GatewayScanDialog.css" />