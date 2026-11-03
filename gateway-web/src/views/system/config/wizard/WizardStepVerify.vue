<template>
  <div>
    <el-descriptions :column="3" border size="small" class="info-line">
      <el-descriptions-item label="设备">{{ deviceName }}</el-descriptions-item>
      <el-descriptions-item label="点位">{{ pointCount }} 个</el-descriptions-item>
      <el-descriptions-item label="上行">{{ uplinkEnabled ? `MQTT ${uplinkTopic}` : '未启用' }}</el-descriptions-item>
    </el-descriptions>

    <p>逐个读取已配置的点位，确认总线、从站地址与数据类型正确。</p>
    <el-button type="primary" :icon="VideoPlay" :loading="verifying" @click="emit('verify')">执行采集测试</el-button>

    <el-alert
      v-if="verifySummary"
      :type="verifyFailed ? 'warning' : 'success'"
      :closable="false"
      show-icon
      :title="verifySummary"
      class="verify-alert"
    />
    <DataTable
      v-if="readResults.length"
      :data="readResults"
      :columns="readColumns"
      row-key="id"
      class="verify-table"
    />
  </div>
</template>

<script setup lang="ts">
/**
 * 部署向导 · 第 5 步：验证采集。
 *
 * 逐个读取已配置的点位，把每条读结果（成功 / 失败 + 数值）如实列出，
 * 失败项不隐藏，便于现场核对从站地址、寄存器地址与数据类型。
 *
 * Props:
 *   - deviceName:    设备名称（只读回显）
 *   - pointCount:    已配置点位数量
 *   - uplinkEnabled: 上行转发是否启用
 *   - uplinkTopic:   上报主题
 *   - verifying:     采集测试进行中（按钮 loading）
 *   - verifySummary: 测试结论文案（为空时不显示结论条）
 *   - verifyFailed:  是否有失败点位（决定结论条是 warning 还是 success）
 *   - readResults:   逐点读取结果
 *
 * Emits:
 *   - verify: 点击「执行采集测试」，由主组件逐点调用真实读取接口
 *
 * 典型应用场景：部署向导的最后一步，只负责渲染与局部交互，不直接发请求。
 */
import { h } from 'vue'
import { ElTag } from 'element-plus'
import { VideoPlay } from '@element-plus/icons-vue'
import DataTable, { type ColumnDef } from '@/components/DataTable.vue'
import type { ReadResult } from './types'

defineProps<{
  deviceName: string
  pointCount: number
  uplinkEnabled: boolean
  uplinkTopic: string
  verifying: boolean
  verifySummary: string
  verifyFailed: boolean
  readResults: ReadResult[]
}>()

const emit = defineEmits<{
  verify: []
}>()

/** 读取结果表列定义 */
const readColumns: ColumnDef<ReadResult>[] = [
  { key: 'name', title: '点位', minWidth: 140, ellipsis: true },
  { key: 'id', title: '点位 ID', minWidth: 180, ellipsis: true },
  {
    key: 'ok',
    title: '结果',
    width: 120,
    cell: (row) =>
      h(ElTag, { type: row.ok ? 'success' : 'danger', size: 'small' }, () =>
        row.ok ? '读取成功' : '读取失败',
      ),
  },
  { key: 'value', title: '数值', width: 140 },
]
</script>

<style scoped>
.info-line { margin-bottom: 16px; }
.verify-alert { margin-top: 16px; }
.verify-table { margin-top: 12px; }
</style>
