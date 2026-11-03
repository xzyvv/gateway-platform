<template>
  <el-form inline @submit.prevent>
    <el-form-item label="总线">
      <el-select v-model="bus" style="width:200px" :loading="loading" placeholder="选择总线" aria-label="回放目标总线" no-data-text="暂无总线，请先在配置管理中添加">
        <el-option v-for="o in busOptions" :key="o.value" :label="o.label" :value="o.value" />
      </el-select>
    </el-form-item>
    <el-form-item label="超时(ms)">
      <el-input-number v-model="timeoutMs" :min="100" :max="10000" :step="100" style="width:120px" aria-label="单帧超时毫秒" />
    </el-form-item>
    <el-form-item label="间隔(ms)">
      <el-input-number v-model="intervalMs" :min="20" :max="5000" :step="20" style="width:120px" aria-label="帧间隔毫秒" />
    </el-form-item>
    <el-form-item>
      <el-checkbox v-model="stopOnMismatch" aria-label="遇不一致即停">遇不一致即停</el-checkbox>
    </el-form-item>
    <el-form-item>
      <el-button size="small" type="primary" @click="emit('replay')" :loading="playing" :disabled="!bus" :icon="VideoPlay">{{ playing ? '回放中...' : '回放' }}</el-button>
      <el-button v-if="playing" size="small" type="danger" plain @click="emit('stop')">停止</el-button>
      <el-button size="small" @click="emit('reset')" :disabled="playing">重置</el-button>
      <el-button size="small" plain @click="emit('save-as-new')" :disabled="playing || !hasSession">保存为新会话</el-button>
    </el-form-item>
  </el-form>
</template>

<script setup lang="ts">
/**
 * 报文回放「执行参数」表单（从 PacketReplay.vue 拆出）
 *
 * 职责：总线、单帧超时、帧间隔、遇不一致即停开关，以及回放/停止/重置/另存入口。
 * 只承接参数的双向绑定与意图上报，不直接调用接口——回放链路的状态与请求
 * 由 usePacketReplay 统一维护，这里点击按钮只发事件。
 *
 * Props:
 * - busOptions：总线选项（来自共享调试目标）
 * - loading：总线选项加载中
 * - playing：回放进行中（按钮切「回放中...」并禁用重置/另存）
 * - hasSession：当前是否打开了某个回放会话
 *
 * Emits:
 * - replay / stop / reset / save-as-new：四个执行入口
 */
import { VideoPlay } from '@element-plus/icons-vue'

defineProps<{
  /** 总线选项 */
  busOptions: Array<{ label: string; value: string }>
  /** 总线选项加载中 */
  loading: boolean
  /** 回放进行中 */
  playing: boolean
  /** 当前是否打开了某个回放会话 */
  hasSession: boolean
}>()

const emit = defineEmits<{
  /** 开始回放 */
  (e: 'replay'): void
  /** 停止回放 */
  (e: 'stop'): void
  /** 重置结果 */
  (e: 'reset'): void
  /** 保存为新会话 */
  (e: 'save-as-new'): void
}>()

/** 回放目标总线（与父页面共享调试目标同一个 ref） */
const bus = defineModel<string>('bus', { required: true })
/** 单帧等待超时（ms） */
const timeoutMs = defineModel<number>('timeoutMs', { required: true })
/** 帧间隔（ms） */
const intervalMs = defineModel<number>('intervalMs', { required: true })
/** 遇不一致即停 */
const stopOnMismatch = defineModel<boolean>('stopOnMismatch', { required: true })
</script>
