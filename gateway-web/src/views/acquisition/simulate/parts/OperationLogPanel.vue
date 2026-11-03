<template>
  <el-card class="log-card" shadow="never">
    <SectionHeader title="操作记录" icon="Document">
      <el-button size="small" text :disabled="!logs.length" @click="emit('export')">
        <el-icon><Download /></el-icon> 导出 CSV
      </el-button>
      <el-button size="small" text @click="emit('clear')">
        <el-icon><Delete /></el-icon> 清空
      </el-button>
    </SectionHeader>
    <div class="log-list">
      <div
        v-for="(log, i) in logs"
        :key="i"
        class="log-item"
        :class="log.type"
      >
        <div class="log-time">{{ log.time }}</div>
        <div class="log-content">
          <el-tag size="small" :type="log.type" effect="dark" class="log-tag">
            {{ log.tag }}
          </el-tag>
          <span class="log-msg">{{ log.message }}</span>
        </div>
        <!-- 原始报文（请求/响应 HEX）：来自抓包缓冲，现场排障可直接复制 -->
        <div v-if="log.frames?.length" class="log-frames">
          <div v-for="(f, fi) in log.frames" :key="fi" class="log-frame">
            <span class="frame-dir" :class="f.direction">{{ f.direction === 'request' ? '请求' : '响应' }}</span>
            <span class="frame-hex">{{ f.hex }}</span>
          </div>
        </div>
      </div>
      <empty-state v-if="!logs.length" description="发送读写请求后，收发的报文与结果会记录在这里" :image-size="80" />
    </div>
  </el-card>
</template>

<script lang="ts">
/** 操作记录条目：一次读写 / 诊断 / 批量写入的结果快照 */
export interface LogEntry {
  time: string
  type: 'info' | 'success' | 'warning' | 'danger'
  tag: string
  message: string
  /** 本次操作时间窗内抓到的原始报文（请求/响应 HEX）；抓包接口不可用时缺省 */
  frames?: { direction: string; hex: string }[]
}
</script>

<script setup lang="ts">
/**
 * 右侧「操作记录」面板（从 Tester.vue 拆出）
 *
 * 职责：按时间倒序展示读写/诊断/批量写入的结果，每条可附原始报文 HEX；
 * 并提供「导出 CSV」「清空」两个操作入口（导出与清空的实际动作在父组件）。
 *
 * Props:
 * - logs：操作记录（最新在前）
 *
 * Emits:
 * - export：导出为 CSV（读写记录是现场排障证据）
 * - clear：清空记录
 *
 * 典型应用场景：点位读写测试页（Tester.vue）右栏。
 */
import { Delete, Download } from '@element-plus/icons-vue'

defineProps<{
  /** 操作记录（最新在前） */
  logs: LogEntry[]
}>()

const emit = defineEmits<{
  /** 导出操作记录 */
  (e: 'export'): void
  /** 清空操作记录 */
  (e: 'clear'): void
}>()
</script>

<style scoped>
/* 以下规则来自原 Tester.css，随区块一起迁移到本组件 */
.log-list {
  max-height: 480px;
  overflow-y: auto;
  min-height: 300px;
}
.log-item {
  padding: 8px 10px;
  border-radius: var(--radius-button);
  margin-bottom: 6px;
  background: var(--bg-fill);
  border-left: 3px solid var(--border-light);
  transition: transform var(--transition-fast), box-shadow var(--transition-fast);
}
.log-item:hover {
  transform: translateX(2px);
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.04);
}
.log-item.success {
  border-left-color: var(--color-success);
  background: var(--color-success-bg);
}
.log-item.warning {
  border-left-color: var(--color-warning);
  background: var(--color-warning-bg);
}
.log-item.danger {
  border-left-color: var(--color-danger);
  background: var(--color-danger-bg);
}
.log-item.info {
  border-left-color: var(--color-primary);
  background: var(--color-primary-bg);
}
.log-time {
  font-size: 0.6875rem;
  color: var(--text-secondary);
  margin-bottom: 3px;
  font-family: 'Consolas', monospace;
  letter-spacing: 0.3px;
}
.log-content {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.8125rem;
  color: var(--text-regular);
}
.log-tag { flex-shrink: 0; }
.log-msg { word-break: break-all; line-height: 1.5; }

/* ===== 原始报文（请求/响应 HEX）：来自抓包缓冲，等宽可换行，便于现场复制 ===== */
.log-frames {
  margin-top: 6px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.log-frame {
  display: flex;
  gap: 8px;
  align-items: baseline;
  font-family: 'Consolas', 'Monaco', monospace;
  font-size: 0.75rem;
}
.frame-dir {
  flex-shrink: 0;
  color: var(--text-secondary);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-small);
  padding: 0 4px;
  font-size: 0.6875rem;
  line-height: 1.6;
}
.frame-dir.request {
  color: var(--color-primary);
  border-color: var(--color-primary);
}
.frame-dir.response {
  color: var(--text-success);
  border-color: var(--text-success);
}
.frame-hex {
  word-break: break-all;
  line-height: 1.5;
  color: var(--text-regular);
  user-select: all;
}
</style>
