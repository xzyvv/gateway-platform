<template>
  <div class="rp">
    <SectionHeader title="报文回放" icon="VideoCamera" />

    <el-alert
      v-if="replayError"
      class="section-card"
      type="error"
      show-icon
      :closable="true"
      :title="replayError"
      @close="replayError = ''"
    />

    <el-row :gutter="16">
      <el-col :span="6">
        <ReplaySessionPanel
          :sessions="sessions"
          :current-id="currentId"
          @select="select"
          @delete="doDelete"
        />
      </el-col>
      <el-col :span="18">
        <el-card v-if="session" shadow="never" class="section-card">
          <template #header>
            <div style="display:flex;justify-content:space-between;align-items:center;flex-wrap:wrap;gap:8px">
              <strong>{{ session.name }}</strong>
              <ReplayControls
                v-model:bus="bus"
                v-model:timeout-ms="timeoutMs"
                v-model:interval-ms="intervalMs"
                v-model:stop-on-mismatch="stopOnMismatch"
                :bus-options="busOptions"
                :loading="targetsLoading"
                :playing="playing"
                :has-session="!!session"
                @replay="replay"
                @stop="stopReplay"
                @reset="reset"
                @save-as-new="saveAsNew"
              />
            </div>
          </template>

          <ReplayRunStatus
            :playing="playing"
            :done-count="liveFrames.length"
            :total="streamTotal"
            :percentage="progressPct"
            :result="execResult"
            :terminal="terminalState"
            :terminal-error="terminalError"
          />
          <ReplayFrameList :packets="session.packets" :frames="framesById" />
        </el-card>
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
/**
 * 报文回放（薄编排层）
 *
 * 状态与回放链路（列表/选中会话、流式逐帧进度、停止/重置/另存）全部在
 * composables/acquisition/usePacketReplay.ts；
 * 展示按区块拆到同目录 components/ 下：
 * - components/ReplaySessionPanel.vue 左侧「回放文件列表」
 * - components/ReplayControls.vue     右上执行参数表单（总线/超时/间隔/按钮）
 * - components/ReplayRunStatus.vue    流式进度条与回放结果汇总
 * - components/ReplayFrameList.vue    逐帧列表与判定详情
 *
 * 本文件只保留布局、事件接线与子组件组合。
 */
import { usePacketReplay } from '@/composables/acquisition/usePacketReplay'
import ReplaySessionPanel from './components/ReplaySessionPanel.vue'
import ReplayControls from './components/ReplayControls.vue'
import ReplayRunStatus from './components/ReplayRunStatus.vue'
import ReplayFrameList from './components/ReplayFrameList.vue'

const {
  busOptions,
  targetsLoading,
  bus,
  sessions,
  currentId,
  session,
  replayError,
  playing,
  timeoutMs,
  intervalMs,
  stopOnMismatch,
  execResult,
  liveFrames,
  streamTotal,
  progressPct,
  terminalState,
  terminalError,
  framesById,
  select,
  doDelete,
  saveAsNew,
  replay,
  stopReplay,
  reset,
} = usePacketReplay()
</script>

<style scoped>
.rp { padding: 0px; }
.section-card { margin-bottom: 16px; }
</style>
