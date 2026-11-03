<template>
  <div>
    <el-descriptions :column="3" border size="small" class="info-line">
      <el-descriptions-item label="总线">{{ selectedBus?.port_name }}</el-descriptions-item>
      <el-descriptions-item label="协议">{{ selectedBus?.protocol }}</el-descriptions-item>
      <el-descriptions-item label="波特率">{{ selectedBus?.baud_rate }}</el-descriptions-item>
    </el-descriptions>

    <el-form label-width="104px" style="max-width: 560px">
      <el-form-item label="设备名称" required>
        <el-input v-model="deviceForm.name" placeholder="如：1 号电表" maxlength="40" />
      </el-form-item>
      <el-form-item label="从站地址" required>
        <el-input-number v-model="deviceForm.slaveId" :min="1" :max="247" controls-position="right" />
        <div class="field-hint">Modbus 从站号 1~247，按设备铭牌或拨码填写。</div>
      </el-form-item>
      <el-form-item label="设备模板" required>
        <el-select v-model="deviceForm.template" placeholder="选择设备模板（必选）" style="width: 100%">
          <el-option
            v-for="t in templates"
            :key="t.name"
            :label="`${t.name}（${t.points?.length ?? 0} 个点位）`"
            :value="t.name"
          />
        </el-select>
        <div class="field-hint">网关要求每台设备绑定模板，模板决定它能读哪些点位。</div>
        <el-button link type="primary" size="small" @click="emit('open-new-template')">
          没有合适的？新建模板
        </el-button>
      </el-form-item>
    </el-form>
  </div>
</template>

<script setup lang="ts">
/**
 * 部署向导 · 第 2 步：新增设备。
 *
 * 顶部回显第 1 步选中的总线信息，下面填设备名称 / 从站地址 / 设备模板。
 * 模板没有合适的时可点击「新建模板」就地创建（由主组件弹出对话框处理）。
 *
 * Props:
 *   - selectedBus: 第 1 步选中的总线（只读回显）
 *   - templates:   网关模板库列表（模板下拉项来源）
 *   - deviceForm:  设备表单值（v-model：name / slaveId / template）
 *
 * Emits:
 *   - open-new-template: 点击「没有合适的？新建模板」
 *
 * 典型应用场景：部署向导的第 2 步，只负责渲染与局部交互，不直接发请求。
 */
import type { BusConfig } from '@/api/types'
import type { DeviceTemplate } from '@/api/templates'
import type { DeviceFormModel } from './types'

defineProps<{
  selectedBus: BusConfig | undefined
  templates: DeviceTemplate[]
}>()

/** 设备表单值（主组件最终组装成 DeviceConfig 并调用创建接口） */
const deviceForm = defineModel<DeviceFormModel>('deviceForm', { required: true })

const emit = defineEmits<{
  'open-new-template': []
}>()
</script>

<style scoped>
.info-line { margin-bottom: 16px; }
.field-hint { font-size: 0.75rem; color: var(--text-secondary); line-height: 1.5; margin-top: 2px; }
</style>
