<template>
  <el-card class="info-card" shadow="never">
    <div class="info-row">
      <el-icon class="info-icon"><Operation /></el-icon>
      <div class="info-text">
        <!-- 单行提示条：现场调试要的是「能不能写、写完去哪看」，不是一段说明书 -->
        <p>
          选设备与点位后可读取或写入测试值，用于验证地址配置与采集链路。
          <el-link v-if="supportsWrite" type="primary" @click="emit('scroll-to-batch-write')">批量写入</el-link>
          支持一次提交多个点位；批量性能测试请用【读取性能】。
        </p>
      </div>
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 点位读写测试页的顶部说明卡（从 Tester.vue 拆出）
 *
 * 职责：只渲染一句话使用说明 + 「批量写入」锚点链接，不持有任何状态，
 * 是否展示写入入口由父组件按协议能力元数据（supports_write）决定。
 *
 * Props:
 * - supportsWrite：当前所选协议的写入能力声明；为 false 时隐藏「批量写入」入口
 *
 * Emits:
 * - scroll-to-batch-write：点击「批量写入」链接时通知父组件滚动到批量写入卡片
 *
 * 典型应用场景：点位读写测试页（Tester.vue）顶部的使用提示条。
 */
import { Operation } from '@element-plus/icons-vue'

defineProps<{
  /** 协议是否声明支持写入（来自 GET /api/protocols 的 supports_write） */
  supportsWrite: boolean
}>()

const emit = defineEmits<{
  /** 点击「批量写入」：父组件负责滚动定位 */
  (e: 'scroll-to-batch-write'): void
}>()
</script>

<style scoped>
/* 以下规则来自原 Tester.css，随区块一起迁移到本组件 */
.info-card .info-row {
  display: flex;
  align-items: flex-start;
  gap: 14px;
}
.info-icon {
  font-size: 1.75rem;
  color: var(--color-primary);
  margin-top: 2px;
  flex-shrink: 0;
}
.info-text h3 {
  margin: 0 0 6px;
  font-size: 1rem;
  color: var(--text-primary);
}
.info-text p {
  margin: 0;
  font-size: 0.8125rem;
  color: var(--text-secondary);
  line-height: 1.6;
}
</style>
