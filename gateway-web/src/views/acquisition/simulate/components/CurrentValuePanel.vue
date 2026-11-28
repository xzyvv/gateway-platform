<template>
  <!-- 当前值展示 -->
  <div class="value-display" v-if="hasPoint">
    <div class="value-label">当前值</div>
    <div class="value-number" :class="{ danger: valueError }">
      <span v-if="valueLoading" class="value-loading">读取中...</span>
      <span v-else-if="valueError" class="value-error">读取失败</span>
      <span v-else>{{ currentValue }}</span>
      <span v-if="valueUnit" class="value-unit">{{ valueUnit }}</span>
    </div>
  </div>

  <!-- 读按钮 -->
  <div class="action-row">
    <el-button
      type="primary"
      :disabled="!hasPoint"
      :loading="valueLoading"
      @click="emit('read')"
    >
      <el-icon><Reading /></el-icon> 读取
    </el-button>
    <!-- 协议诊断：后端 Collector::diag（s7comm 读 SZL / bacnet 发 Who-Is / iec104 建链+总召唤 / gpio 读芯片）
         此前只有能力位没有入口。诊断与采集争用同一链路，后端对未暂停的总线会
         拒绝执行并说明原因，这里只如实记录，不替用户自动暂停采集 -->
    <el-button
      :disabled="!supportsDiag"
      :loading="diagLoading"
      @click="emit('diag')"
    >
      <el-icon><FirstAidKit /></el-icon> 协议诊断
    </el-button>
  </div>
</template>

<script setup lang="ts">
/**
 * 当前值展示 + 读取/协议诊断操作区（从 Tester.vue 拆出）
 *
 * 职责：展示点位当前值（加载中/失败/正常三种态），并提供「读取」与「协议诊断」
 * 两个操作入口；具体请求逻辑仍在父组件（Tester.vue），本组件只负责展示与抛事件。
 *
 * Props:
 * - hasPoint：是否已选择点位（未选点位时隐藏当前值并禁用读取按钮）
 * - currentValue：最近一次读取到的值
 * - valueLoading / valueError：读取中 / 读取失败标记
 * - valueUnit：点位单位（数据类型或协议名），为空则不展示
 * - diagLoading：协议诊断进行中
 * - supportsDiag：协议是否声明支持诊断（能力未知时按不支持处理）
 *
 * Emits:
 * - read：点击「读取」
 * - diag：点击「协议诊断」
 *
 * 典型应用场景：点位读写测试页（Tester.vue）的「选择点位」卡片中下部。
 */
import { FirstAidKit, Reading } from '@element-plus/icons-vue'

defineProps<{
  /** 是否已选择点位 */
  hasPoint: boolean
  /** 最近一次读取到的点位值 */
  currentValue: number | string
  /** 读取进行中 */
  valueLoading: boolean
  /** 上一次读取是否失败 */
  valueError: boolean
  /** 点位单位（数据类型或协议名） */
  valueUnit: string
  /** 协议诊断进行中 */
  diagLoading: boolean
  /** 协议是否声明支持诊断 */
  supportsDiag: boolean
}>()

const emit = defineEmits<{
  /** 发起一次点位读取 */
  (e: 'read'): void
  /** 发起一次协议诊断 */
  (e: 'diag'): void
}>()
</script>

<style scoped>
/* 以下规则来自原 Tester.css，随区块一起迁移到本组件 */
.value-display {
  background: var(--bg-fill);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-card);
  padding: 18px;
  text-align: center;
  margin-top: 8px;
}
.value-label {
  font-size: 0.75rem;
  color: var(--text-secondary);
  margin-bottom: 6px;
}
.value-number {
  font-size: 2rem;
  font-weight: 700;
  color: var(--color-primary);
  font-family: 'Consolas', 'Monaco', monospace;
  line-height: 1.2;
}
.value-number.danger { color: var(--text-danger); }
.value-unit {
  font-size: 0.8125rem;
  font-weight: 400;
  color: var(--text-secondary);
  margin-left: 6px;
}
.value-loading { font-size: 1rem; color: var(--text-secondary); }

.action-row {
  display: flex;
  gap: 8px;
  margin-top: 14px;
  flex-wrap: wrap;
}
</style>
