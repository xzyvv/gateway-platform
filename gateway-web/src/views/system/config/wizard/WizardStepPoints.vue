<template>
  <div>
    <el-descriptions :column="3" border size="small" class="info-line">
      <el-descriptions-item label="设备">{{ deviceName }}</el-descriptions-item>
      <el-descriptions-item label="总线">{{ selectedBus?.port_name }}</el-descriptions-item>
      <el-descriptions-item label="设备地址">{{ deviceAddress }}</el-descriptions-item>
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
      <el-alert
        v-if="!isModbusBus"
        type="info"
        :closable="false"
        show-icon
        class="point-tip"
        title="当前总线不是 Modbus"
        description="DLT645 / CJT188 等协议按数据标识寻址，向导不支持按寄存器地址补录，请完成向导后在「点位管理」中添加。"
      />
      <el-form v-else label-width="104px" style="max-width: 560px" class="point-form">
        <el-form-item label="点位名称">
          <el-input v-model="manual.name" placeholder="如：电压" maxlength="32" />
        </el-form-item>
        <el-form-item label="寄存器地址">
          <el-input v-model="manual.address" placeholder="40001" maxlength="5" />
          <div class="point-hint">
            5 位地址 = 区段首位 + 1 基序号：0xxxx 线圈 / 1xxxx 离散输入 / 3xxxx 保持寄存器 / 4xxxx 输入寄存器。
            区段决定功能码，采集按其换算出偏移，填错区段会读错寄存器。
          </div>
        </el-form-item>
        <el-form-item label="数据类型">
          <el-select v-model="manual.dataType" style="width: 100%">
            <el-option v-for="t in DATA_TYPES" :key="t" :label="t" :value="t" />
          </el-select>
        </el-form-item>
      </el-form>
      <div class="step-actions">
        <el-button type="primary" :loading="addingPoint" :disabled="!isModbusBus" @click="emit('add-point')">添加点位</el-button>
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
 *   - manual:        手动补录点位的表单值（v-model：name / address / dataType）
 *   - addingPoint:   添加点位请求进行中（按钮 loading）
 *
 * Emits:
 *   - add-point: 点击「添加点位」，由主组件调用真实接口写入点位
 *
 * 典型应用场景：部署向导的第 3 步，只负责渲染与局部交互，不直接发请求。
 */
import { computed } from 'vue'
import DataTable, { type ColumnDef } from '@/components/DataTable.vue'
import { addressColumn } from '@/utils/commonColumns'
import { DATA_TYPES } from '@/composables/devices/decodeParams'
import type { BusConfig, PointConfig } from '@/api/types'
import type { ManualPointForm } from './types'

const props = defineProps<{
  deviceName: string
  selectedBus: BusConfig | undefined
  deviceAddress: string | undefined
  points: PointConfig[]
  loadingPoints: boolean
  addingPoint: boolean
}>()

/** 手动补录点位的表单值 */
const manual = defineModel<ManualPointForm>('manual', { required: true })

/**
 * 本步的补录表单只适用于 Modbus：DLT645 / CJT188 按数据标识、FINS / MC 按软元件寻址，
 * 都不是「寄存器地址 + 功能码」这套模型。非 Modbus 时隐藏表单，避免建出采不到数的点位。
 */
const isModbusBus = computed(() =>
  (props.selectedBus?.protocol ?? '').toLowerCase().startsWith('modbus'),
)

const emit = defineEmits<{
  'add-point': []
}>()

/** 已有点位列表列定义 */
const pointColumns: ColumnDef<PointConfig>[] = [
  { key: 'name', title: '点位名称', minWidth: 140, ellipsis: true },
  { key: 'id', title: '点位 ID', minWidth: 160, ellipsis: true },
  addressColumn<PointConfig>({ title: '点位地址', width: 100 }),
  { key: 'data_type', title: '类型', width: 90 },
  { key: 'interval', title: '周期(s)', width: 90 },
]
</script>

<style scoped>
.info-line { margin-bottom: 16px; }
.point-form { margin-top: 16px; }
.point-tip { margin-top: 16px; }
.point-hint { font-size: 0.8125rem; color: var(--text-secondary); line-height: 1.6; margin-top: 4px; }
.loading-line { color: var(--text-secondary); font-size: 0.875rem; padding: 12px 0; }
.step-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; padding-top: 16px; border-top: 1px solid var(--el-border-color-lighter); }
</style>
