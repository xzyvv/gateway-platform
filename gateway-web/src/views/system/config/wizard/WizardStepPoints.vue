<template>
  <div>
    <el-descriptions :column="3" border size="small" class="info-line">
      <el-descriptions-item label="设备">{{ deviceName }}</el-descriptions-item>
      <el-descriptions-item label="总线">{{ selectedBus?.port_name }}</el-descriptions-item>
      <el-descriptions-item label="地址">{{ deviceAddress }}</el-descriptions-item>
    </el-descriptions>

    <div v-if="loadingPoints" class="loading-line">正在加载该设备的点位…</div>
    <template v-else>
      <DataTable
        v-if="points.length"
        :data="points"
        :columns="pointColumns"
        row-key="id"
        stripe
        empty-text="暂无点位"
      />
      <el-form label-width="104px" style="max-width: 560px" class="point-form">
        <el-form-item label="点位名称">
          <el-input v-model="manual.name" placeholder="如：电压" maxlength="32" />
        </el-form-item>
        <el-form-item label="寄存器地址">
          <el-input-number v-model="manual.addressNum" :min="0" :max="65535" controls-position="right" />
        </el-form-item>
        <el-form-item label="数据类型">
          <el-select v-model="manual.dataType" style="width: 100%">
            <el-option v-for="t in DATA_TYPES" :key="t" :label="t" :value="t" />
          </el-select>
        </el-form-item>
      </el-form>
      <div class="step-actions">
        <el-button type="primary" :loading="addingPoint" @click="emit('add-point')">添加点位</el-button>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
/**
 * 部署向导 · 第 3 步：配置点位。
 *
 * 顶部回显设备 / 总线 / 地址，中部列出该设备已有点位，底部是手动补录点位的表单。
 *
 * Props:
 *   - deviceName:    设备名称（第 2 步填的名字，只读回显）
 *   - selectedBus:   第 1 步选中的总线（只读回显）
 *   - deviceAddress: 已创建设备的地址（只读回显）
 *   - points:        该设备已有点位列表
 *   - loadingPoints: 点位列表加载中（显示加载文案而不是表格）
 *   - manual:        手动补录点位的表单值（v-model：name / addressNum / dataType）
 *   - addingPoint:   添加点位请求进行中（按钮 loading）
 *
 * Emits:
 *   - add-point: 点击「添加点位」，由主组件调用真实接口写入点位
 *
 * 典型应用场景：部署向导的第 3 步，只负责渲染与局部交互，不直接发请求。
 */
import DataTable, { type ColumnDef } from '@/components/DataTable.vue'
import { addressColumn } from '@/utils/commonColumns'
import type { BusConfig, PointConfig } from '@/api/types'
import type { ManualPointForm } from './types'

defineProps<{
  deviceName: string
  selectedBus: BusConfig | undefined
  deviceAddress: string | undefined
  points: PointConfig[]
  loadingPoints: boolean
  addingPoint: boolean
}>()

/** 手动补录点位的表单值 */
const manual = defineModel<ManualPointForm>('manual', { required: true })

const emit = defineEmits<{
  'add-point': []
}>()

/** 可选的数据类型（与网关侧支持的一致） */
const DATA_TYPES = ['int16', 'uint16', 'int32', 'uint32', 'float', 'double']

/** 已有点位列表列定义 */
const pointColumns: ColumnDef<PointConfig>[] = [
  { key: 'name', title: '点位名称', minWidth: 140, ellipsis: true },
  { key: 'id', title: '点位 ID', minWidth: 160, ellipsis: true },
  addressColumn<PointConfig>({ width: 100 }),
  { key: 'data_type', title: '类型', width: 90 },
  { key: 'interval', title: '周期(s)', width: 90 },
]
</script>

<style scoped>
.info-line { margin-bottom: 16px; }
.point-form { margin-top: 16px; }
.loading-line { color: var(--text-secondary); font-size: 0.875rem; padding: 12px 0; }
.step-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; padding-top: 16px; border-top: 1px solid var(--el-border-color-lighter); }
</style>
