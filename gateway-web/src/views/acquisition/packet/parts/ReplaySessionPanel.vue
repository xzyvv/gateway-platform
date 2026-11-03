<template>
  <el-card shadow="never" class="section-card">
    <template #header><strong>回放文件列表</strong></template>
    <div v-for="s in sessions" :key="s.id" class="session-item" :class="{ active: currentId === s.id, builtin: isBuiltin(s.id) }" role="button" tabindex="0" :aria-label="`回放会话 ${s.name}`" @click="emit('select', s.id)" @keydown.enter="emit('select', s.id)">
      <div class="sname">{{ s.name }} <span v-if="isBuiltin(s.id)" class="builtin-tag">内置</span></div>
      <div class="smeta">{{ s.deviceName }} 共 {{ s.packetCount }}帧 时长 {{ (s.durationMs/1000).toFixed(1) }}s</div>
      <el-button v-if="!isBuiltin(s.id)" class="del-btn" link type="danger" size="small" :aria-label="`删除会话 ${s.name}`" @click.stop="emit('delete', s.id)">删</el-button>
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 报文回放「文件列表」面板（从 PacketReplay.vue 拆出）
 *
 * 职责：列出可回放的录制会话、标出当前选中项与内置项，并提供单选/删除入口。
 *
 * 与页面主组件的分工：本面板不持有任何会话数据，选中与删除都以事件上报，
 * 由 usePacketReplay 统一维护列表与当前会话，避免两处各存一份造成选中态漂移。
 *
 * 预置会话（s1/s2/s3）不可删：后端也拒绝，前端在此隐藏删除按钮。
 *
 * Props:
 * - sessions：回放会话列表
 * - currentId：当前选中的会话 id
 *
 * Emits:
 * - select：用户点击某个会话
 * - delete：用户点击删除（仅非内置会话）
 *
 * 典型应用场景：报文回放页（PacketReplay.vue）左栏的「回放文件列表」卡片。
 */
import type { ReplaySession } from '@/api/protocolTools'
import { isBuiltin } from '@/composables/acquisition/usePacketReplay'

defineProps<{
  /** 回放会话列表 */
  sessions: ReplaySession[]
  /** 当前选中的会话 id */
  currentId: string
}>()

const emit = defineEmits<{
  /** 用户点击某个会话 */
  (e: 'select', id: string): void
  /** 用户点击删除（仅非内置会话） */
  (e: 'delete', id: string): void
}>()
</script>

<style scoped>
.section-card { margin-bottom: 16px; }
.session-item { padding: 10px; border: 1px solid var(--el-border-color); border-radius: var(--radius-button); margin-bottom: 8px; cursor: pointer; }
.session-item:hover { background: var(--el-fill-color-light); }
.session-item.active { border-color: var(--el-color-primary); background: var(--el-color-primary-light-9); }
.sname { font-weight: 600; }
.smeta { font-size: 0.75rem; color: var(--el-text-color-secondary); margin-top: 2px; }
</style>
