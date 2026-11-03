<template>
  <div class="toolbar">
    <el-select
      :model-value="bus"
      :loading="busLoading"
      class="bus-select"
      placeholder="选择总线"
      aria-label="总线"
      :disabled="running"
      @change="emit('bus-change', $event)"
    >
      <el-option v-for="o in busOptions" :key="o.value" :label="o.label" :value="o.value" />
    </el-select>
    <el-select
      :model-value="activeId"
      class="seq-select"
      aria-label="调试序列"
      :disabled="running"
      @change="emit('select-sequence', $event)"
    >
      <el-option v-for="s in sequences" :key="s.id" :label="s.name" :value="s.id" />
    </el-select>
  </div>

  <!-- E1：任一步因总线被采集占用而失败时，直接给出「暂停采集」入口 -->
  <BusPauseBanner :bus="bus" :visible="busBusy" />

  <div v-if="hasSequence" class="seq-meta">
    <el-input
      :model-value="seqName"
      size="small"
      class="seq-name"
      placeholder="序列名"
      aria-label="序列名"
      :disabled="running"
      @change="emit('rename-sequence', $event)"
    />
    <el-tag size="small" type="info">完成 {{ doneCount }}/{{ totalCount }}</el-tag>
    <el-checkbox v-model="stopOnFail" :disabled="running">失败即停</el-checkbox>
  </div>
</template>

<script setup lang="ts">
/**
 * 单步调试「序列工具条」区块（从 StepSequencePanel.vue 拆出）
 *
 * 职责：总线选择、序列选择、序列重命名、进度展示与「失败即停」开关，
 * 以及总线被采集占用时的「暂停采集」入口（E1）。
 *
 * 本区块是纯展示 + 意图上报：所有会改动序列数据的动作（换总线、写回总线名、
 * 切换序列、重命名）都由父面板在收到事件后统一执行，保证序列状态只有一处维护。
 *
 * Props:
 * - busOptions：总线选项（来自 GET /api/config 的 buses）
 * - busLoading：总线选项加载中
 * - running：是否正在执行（执行中禁用编辑）
 * - outcomes：每步执行结果，用于「完成 x/y」进度与总线占用判定
 * - sequences：可选序列列表（id + name）
 * - activeId：当前选中的序列 id
 * - bus：当前选中的总线
 * - hasSequence：是否存在当前序列（无序列时不渲染序列信息行）
 * - seqName：当前序列名
 * - doneCount：已完成步骤数
 * - totalCount：步骤总数
 * - stopOnFail：失败即停开关（v-model）
 *
 * Emits:
 * - bus-change：用户切换总线（父面板负责写回序列总线并清执行进度）
 * - select-sequence：用户切换序列（父面板负责切换并清执行进度）
 * - rename-sequence：用户重命名序列
 *
 * 典型应用场景：报文分步调试页（StepDebug.vue）左栏「自定义序列」卡片的头部工具条。
 */
import { computed } from 'vue'
import BusPauseBanner from '@/components/BusPauseBanner.vue'
import type { StepOutcome } from './stepDebugTypes'

const props = defineProps<{
  /** 总线选项 */
  busOptions: Array<{ label: string; value: string }>
  /** 总线选项加载中 */
  busLoading: boolean
  /** 是否正在执行 */
  running: boolean
  /** 每步执行结果（主组件维护） */
  outcomes: Record<string, StepOutcome>
  /** 可选序列列表 */
  sequences: Array<{ id: string; name: string }>
  /** 当前选中的序列 id */
  activeId: string
  /** 当前选中的总线 */
  bus: string
  /** 是否存在当前序列 */
  hasSequence: boolean
  /** 当前序列名 */
  seqName: string
  /** 已完成步骤数 */
  doneCount: number
  /** 步骤总数 */
  totalCount: number
}>()

const emit = defineEmits<{
  /** 用户切换总线 */
  (e: 'bus-change', bus: string): void
  /** 用户切换序列 */
  (e: 'select-sequence', id: string): void
  /** 用户重命名序列 */
  (e: 'rename-sequence', name: string): void
}>()

/** 失败即停开关（与父面板双向绑定） */
const stopOnFail = defineModel<boolean>('stopOnFail', { required: true })

/** E1：任一步因「总线被采集占用」失败 → 展示暂停采集入口，而不是让用户反复点运行 */
const busBusy = computed(() => Object.values(props.outcomes).some((o) => o.error === 'bus_busy'))
</script>

<style scoped>
.toolbar { display: flex; gap: 8px; margin-bottom: 10px; }
.bus-select { width: 200px; }
.seq-select { flex: 1; }
.seq-meta { display: flex; gap: 8px; align-items: center; margin-bottom: 10px; }
.seq-name { width: 180px; }
</style>
