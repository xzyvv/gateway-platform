<template>
  <el-form label-position="top" class="tester-form">
    <!-- 设备与点位并排：两组都是「选完就不用再改」的一次性输入，竖排会白吃掉 70px 高度 -->
    <div class="form-row">
      <el-form-item label="设备">
        <el-select
          v-model="selectedDevice"
          placeholder="请选择设备"
          filterable
          aria-label="设备"
          :loading="devicesLoading"
          style="width: 100%"
        >
          <el-option
            v-for="d in devices"
            :key="d.id"
            :label="`${d.name || d.id} (${d.protocol})`"
            :value="d.id"
          />
        </el-select>
        <CapabilityBadges
          v-if="selectedProtocol"
          :caps-known="capsKnown"
          :supports-write="supportsWrite"
          :supports-batch-read="supportsBatchRead"
          :supports-scan-slaves="supportsScanSlaves"
          :supports-diag="supportsDiag"
        />
      </el-form-item>

      <el-form-item label="点位">
        <el-select
          v-model="selectedPoint"
          placeholder="请选择点位"
          filterable
          :disabled="!selectedDevice"
          :loading="pointsLoading"
          style="width: 100%"
          @change="emit('point-change')"
        >
          <el-option
            v-for="p in points"
            :key="p.id"
            :label="`${p.name || p.id} (${p.data_type || p.protocol || ''})`"
            :value="p.id"
          />
        </el-select>
      </el-form-item>
    </div>
  </el-form>
</template>

<script setup lang="ts">
/**
 * 设备与点位选择区（从 Tester.vue 拆出）
 *
 * 职责：渲染设备下拉 + 点位下拉 + 协议能力徽标，并把选择结果回传父组件。
 * 设备/点位的数据加载与点位请求序号（Bug 181）仍由父组件负责，
 * 本组件只负责「选中什么」。
 *
 * Props:
 * - devices：设备列表；selectedDevice：当前设备 id（v-model）
 * - points：点位列表；selectedPoint：当前点位 id（v-model）
 * - devicesLoading / pointsLoading：下拉加载态
 * - selectedProtocol：所选设备对应的协议名（为空时不渲染能力徽标）
 * - capsKnown / supportsWrite / supportsBatchRead / supportsScanSlaves / supportsDiag：协议能力声明
 *
 * Emits:
 * - update:selectedDevice / update:selectedPoint：选择变化（v-model）
 * - point-change：点位由用户切换（父组件据此清空上一轮的当前值与待写值）
 *
 * 典型应用场景：点位读写测试页（Tester.vue）的「选择点位」卡片。
 */
import CapabilityBadges from './CapabilityBadges.vue'
import type { DeviceConfig, PointConfig } from '@/api/types'

defineProps<{
  /** 设备列表 */
  devices: DeviceConfig[]
  /** 点位列表（当前设备下的点位） */
  points: PointConfig[]
  /** 设备下拉加载态 */
  devicesLoading: boolean
  /** 点位下拉加载态 */
  pointsLoading: boolean
  /** 所选设备对应的协议名；为空表示尚未选择设备 */
  selectedProtocol: string
  /** 所选协议的能力元数据是否已知 */
  capsKnown: boolean
  /** 是否支持写入 */
  supportsWrite: boolean
  /** 是否支持批量读 */
  supportsBatchRead: boolean
  /** 是否支持扫描从站 */
  supportsScanSlaves: boolean
  /** 是否支持协议诊断 */
  supportsDiag: boolean
}>()

/** 当前设备 id（与父组件双向绑定） */
const selectedDevice = defineModel<string>('selectedDevice', { required: true })
/** 当前点位 id（与父组件双向绑定） */
const selectedPoint = defineModel<string>('selectedPoint', { required: true })

const emit = defineEmits<{
  /** 用户切换点位：父组件需清空上一轮的当前值与待写值 */
  (e: 'point-change'): void
}>()
</script>

<style scoped>
/* 以下规则来自原 Tester.css，随区块一起迁移到本组件 */
.tester-form {
  margin-top: 8px;
}
/* 设备 / 点位并排；窄屏（笔记本 1366 下右栏并排时仍有 300px+）保持两列不折行 */
.form-row {
  display: flex;
  gap: 12px;
}
.form-row > .el-form-item {
  flex: 1 1 0;
  min-width: 0;
  margin-bottom: 12px;
}
</style>
