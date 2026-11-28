<template>
  <log-panel
    title="执行日志"
    icon="Document"
    :empty="logs.length === 0"
    empty-text="点击「单步执行」开始调试"
    copyable
    exportable
    clearable
    :max-height="560"
    :empty-min-height="160"
    @copy-all="emit('copy-all')"
    @export="emit('export')"
    @clear="emit('clear')"
  >
    <div v-for="l in logs" :key="l.key" class="log-item" :class="l.status">
      <div class="log-head">
        <el-tag size="small" :type="l.status === 'ok' ? 'success' : 'danger'">
          步骤{{ l.step }} {{ l.status === 'ok' ? '成功' : '失败' }}
        </el-tag>
        <span class="log-head-right">
          <span class="log-bus">{{ l.bus || '-' }}</span>
          <span class="log-time">{{ formatTimeOfDay(l.at) }}</span>
          <el-button size="small" link @click="emit('copy', l)">复制</el-button>
        </span>
      </div>
      <div class="log-body">
        <div>请求: <code>{{ l.requestHex }}</code></div>
        <div v-if="l.responseHex">响应: <code>{{ l.responseHex }}</code></div>
        <div v-if="l.parsed">值: <b>{{ l.parsed }}</b></div>
        <div v-if="l.latencyMs">耗时: {{ l.latencyMs }}ms</div>
        <div v-if="l.error" class="error">错误: {{ errorLabel(l.error) }}</div>
      </div>
    </div>
  </log-panel>
</template>

<script setup lang="ts">
/**
 * 单步调试「执行日志」面板（从 StepDebug.vue 拆出）
 *
 * 职责：按执行顺序展示每一步的请求/响应 HEX、解析值、耗时与错误原因，
 * 并提供单条复制、复制全部、导出 CSV、清空四个操作入口（实际动作在页面主组件）。
 *
 * 卡片壳与「复制全部 / 导出 / 清空」三件套由 components/LogPanel.vue 统一渲染；
 * 本组件只负责日志条目的排版。此前这里缺「导出」——要按耗时排序分析一次
 * 长序列，只能一条条点「复制」。
 *
 * Props:
 * - logs：执行日志（最新在前）
 *
 * Emits:
 * - copy：复制某一条日志
 * - copy-all：复制全部日志
 * - export：导出全部日志为 CSV
 * - clear：清空日志
 *
 * 典型应用场景：报文分步调试页（StepDebug.vue）右栏的执行日志卡片。
 */
import LogPanel from '@/components/LogPanel.vue'
import { formatTimeOfDay } from '@/utils/datetime'
import { errorLabel, type RunLog } from './stepDebugTypes'

defineProps<{
  /** 执行日志（最新在前） */
  logs: RunLog[]
}>()

const emit = defineEmits<{
  /** 复制单条日志 */
  (e: 'copy', log: RunLog): void
  /** 复制全部日志 */
  (e: 'copy-all'): void
  /** 导出全部日志为 CSV */
  (e: 'export'): void
  /** 清空日志 */
  (e: 'clear'): void
}>()
</script>

<style scoped>
.log-item { padding: 10px; border-left: 3px solid var(--el-color-success); background: var(--el-fill-color-light); margin-bottom: 8px; border-radius: 0 6px 6px 0; }
.log-item.fail { border-color: var(--el-color-danger); }
.log-head { display: flex; justify-content: space-between; margin-bottom: 6px; }
.log-head-right { display: flex; align-items: center; gap: 6px; }
.log-bus { font-size: 0.75rem; color: var(--el-text-color-secondary); }
.log-time { font-size: 0.75rem; color: var(--el-text-color-secondary); }
.log-body { font-size: 0.8125rem; line-height: 1.8; }
.log-body code { color: var(--el-color-primary); }
.error { color: var(--text-danger); }
</style>
