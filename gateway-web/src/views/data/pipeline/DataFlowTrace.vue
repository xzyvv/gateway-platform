<template>
  <div class="dft-page">
    <el-card shadow="never" class="dft-header-card">
      <div class="dft-header">
        <div class="dft-title">
          <el-icon class="dft-title-icon"><DataLine /></el-icon>
          <div>
            <h3>追踪条件</h3>
            <p>选择设备与点位，追踪数据从采集到上送的完整链路。</p>
          </div>
        </div>
        <div class="dft-selectors">
          <el-select v-model="selectedDevice" placeholder="选择设备" aria-label="选择设备" filterable class="dft-sel" :loading="loadingDevices" @change="onDeviceChange">
            <el-option v-for="d in devices" :key="d.id" :label="d.name" :value="d.id" />
          </el-select>
          <el-select
            v-model="selectedPoint"
            :placeholder="loadingPoints ? '加载点位中…' : (selectedDevice ? '请选择点位' : '请先选择设备')"
            aria-label="选择点位"
            filterable class="dft-sel"
            :disabled="!points.length && !loadingPoints"
            :loading="loadingPoints"
          >
            <el-option v-for="p in points" :key="p.id" :label="p.name || p.id" :value="p.id" />
          </el-select>
          <el-button size="small" @click="resetTrace" :disabled="!readResult && !renderedPayload && !sendResult && !readError">
            <el-icon><Refresh /></el-icon> 重置链路
          </el-button>
        </div>
      </div>
    </el-card>

    <div v-if="selectedDevice" class="dft-timeline">
      <div class="dft-step" :class="stepStatus[0]">
        <div class="dft-step-head">
          <div class="dft-step-num">
            <span aria-hidden="true">{{ stepStatus[0] === 'done' ? '' : '1' }}</span>
            <span class="sr-only">{{ stepStatus[0] === 'done' ? '已完成' : '步骤 1' }}</span>
          </div>
          <div class="dft-step-title">设备配置</div>
          <el-tag v-if="device" size="small" type="info">{{ device.protocol }}</el-tag>
        </div>
        <div class="dft-step-body" v-if="device">
          <el-descriptions :column="2" border size="small">
            <el-descriptions-item label="设备名">{{ device.name || '-' }}</el-descriptions-item>
            <el-descriptions-item label="设备 ID">{{ device.id || '-' }}</el-descriptions-item>
            <el-descriptions-item label="总线">{{ device.bus || '-' }}</el-descriptions-item>
            <el-descriptions-item label="地址">{{ device.address || '-' }}</el-descriptions-item>
            <el-descriptions-item label="协议">{{ device.protocol || '-' }}</el-descriptions-item>
            <el-descriptions-item label="模板"><span class="dft-nowrap" :title="device.template || '-'">{{ device.template || '-' }}</span></el-descriptions-item>
          </el-descriptions>
        </div>
        <div v-else class="dft-hint">未找到设备配置</div>
      </div>

      <div v-if="selectedPoint" class="dft-step" :class="stepStatus[1]">
        <div class="dft-step-head">
          <div class="dft-step-num">
            <span aria-hidden="true">{{ stepStatus[1] === 'done' ? '' : '2' }}</span>
            <span class="sr-only">{{ stepStatus[1] === 'done' ? '已完成' : '步骤 2' }}</span>
          </div>
          <div class="dft-step-title">点位配置</div>
          <el-tag v-if="pointInfo && pointInfo.data_type" size="small" type="info">{{ pointInfo.data_type }}</el-tag>
        </div>
        <div class="dft-step-body" v-if="pointInfo">
          <el-descriptions :column="2" border size="small">
            <el-descriptions-item label="点位名">{{ pointInfo.name || pointInfo.id || '-' }}</el-descriptions-item>
            <el-descriptions-item label="点位 ID">{{ pointInfo.id || '-' }}</el-descriptions-item>
            <el-descriptions-item label="寄存器">{{ pointInfo.address || '-' }}</el-descriptions-item>
            <el-descriptions-item label="从站号">{{ pointInfo.slave_id ?? '-' }}</el-descriptions-item>
            <el-descriptions-item label="数据类型">{{ pointInfo.data_type || '-' }}</el-descriptions-item>
            <el-descriptions-item label="缩放">{{ pointInfo.scale ?? 1 }}</el-descriptions-item>
          </el-descriptions>
        </div>
      </div>
      <div v-else-if="loadPointsError" class="dft-hint dft-hint-block dft-points-error">
        <div>点位加载失败：{{ loadPointsError }}</div>
        <el-button size="small" type="primary" link @click="onDeviceChange">重试</el-button>
      </div>
      <div v-else-if="loadingPoints" class="dft-hint dft-hint-block">
        <el-icon class="dft-loading-icon"><Loading /></el-icon>
        <span>正在加载点位列表…</span>
      </div>
      <div v-else class="dft-hint dft-hint-block">该设备没有运行中的点位（含模板展开点位）；请确认设备采集已启动，或在「点位管理」单独新增</div>

      <div v-if="selectedPoint" class="dft-step" :class="stepStatus[2]">
        <div class="dft-step-head">
          <div class="dft-step-num">
            <span aria-hidden="true">{{ stepStatus[2] === 'done' ? '' : '3' }}</span>
            <span class="sr-only">{{ stepStatus[2] === 'done' ? '已完成' : '步骤 3' }}</span>
          </div>
          <div class="dft-step-title">采集执行</div>
          <el-button size="small" type="primary" :loading="reading" @click="doRead" :disabled="stepStatus[2] === 'pending'">立即读取</el-button>
        </div>
        <div class="dft-step-body">
          <div v-if="readResult" class="dft-metric">
            <div class="dft-metric-value" :class="{ bad: !isGood, empty: readResult.value == null }">{{ readResult.value ?? '-' }}</div>
            <div class="dft-metric-meta">
              <el-tag :type="isGood ? 'success' : 'danger'" size="small">{{ readResult.status }}</el-tag>
              <span class="dft-latency" :class="latencyLevel">{{ readLatency }} ms</span>
            </div>
          </div>
          <div v-else-if="readError" class="dft-read-error">
            <el-icon class="dft-error-icon"><WarningFilled /></el-icon>
            <div class="dft-error-text">采集失败：{{ readError }}</div>
          </div>
          <div v-else class="dft-hint">点击「立即读取」触发采集</div>
        </div>
      </div>

      <div v-if="selectedPoint" class="dft-step" :class="stepStatus[3]">
        <div class="dft-step-head">
          <div class="dft-step-num">
            <span aria-hidden="true">{{ stepStatus[3] === 'done' ? '' : '4' }}</span>
            <span class="sr-only">{{ stepStatus[3] === 'done' ? '已完成' : '步骤 4' }}</span>
          </div>
          <div class="dft-step-title">数据转换（模板渲染）</div>
          <el-button size="small" :loading="rendering" @click="doRender" :disabled="stepStatus[3] === 'pending'">预览渲染</el-button>
        </div>
        <div class="dft-step-body">
          <div v-if="renderedPayload" class="dft-pre-wrap">
            <div class="dft-pre-toolbar">
              <span class="dft-pre-label">上行报文</span>
              <el-button size="small" :loading="copying" @click="copyPayload">
                <el-icon><CopyDocument /></el-icon> 复制报文
              </el-button>
            </div>
            <pre class="dft-pre">{{ renderedPayload }}</pre>
          </div>
          <div v-else-if="renderError" class="dft-read-error">
            <el-icon class="dft-error-icon"><WarningFilled /></el-icon>
            <div class="dft-error-text">{{ renderError }}</div>
          </div>
          <div v-else-if="stepStatus[3] === 'pending'" class="dft-hint">请先完成采集</div>
          <div v-else class="dft-hint">读取点位后点击「预览渲染」查看上行报文</div>
        </div>
      </div>

      <div v-if="selectedPoint" class="dft-step" :class="stepStatus[4]">
        <div class="dft-step-head">
          <div class="dft-step-num">
            <span aria-hidden="true">{{ stepStatus[4] === 'done' ? '' : '5' }}</span>
            <span class="sr-only">{{ stepStatus[4] === 'done' ? '已完成' : '步骤 5' }}</span>
          </div>
          <div class="dft-step-title">上送发送</div>
          <el-button size="small" type="success" :loading="sending" @click="doSend" :disabled="stepStatus[4] === 'pending'">
            测试上送
          </el-button>
        </div>
        <div class="dft-step-body">
          <div v-if="sendResult" class="dft-result">
            <div class="dft-result-header">
              <el-icon v-if="sendResult.ok" class="dft-result-ok"><CircleCheckFilled /></el-icon>
              <el-icon v-else class="dft-result-fail"><CircleCloseFilled /></el-icon>
              <span class="dft-result-title">{{ sendResult.ok ? '上送成功' : '上送失败' }}</span>
            </div>
            <div v-if="sendResult.publish" class="dft-publish-tags">
              <el-tag size="small" type="info">topic: {{ sendResult.publish.topic }}</el-tag>
              <el-tag size="small" type="info">qos: {{ sendResult.publish.qos }}</el-tag>
              <el-tag size="small" :type="sendResult.publish.delivered ? 'success' : 'warning'">
                {{ sendResult.publish.delivered ? 'broker 已确认' : '未确认' }}
              </el-tag>
            </div>
            <div v-else-if="sendResult.error" class="dft-send-error">{{ sendResult.error }}</div>
          </div>
          <div v-else-if="stepStatus[4] === 'pending'" class="dft-hint">请先完成渲染</div>
          <div v-else class="dft-hint">渲染报文后点击「测试上送」验证上行链路</div>
        </div>
      </div>
    </div>

    <div v-else-if="loadingDevices" class="dft-page-loading">
      <el-icon class="dft-loading-icon"><Loading /></el-icon>
      <span>正在加载设备列表…</span>
    </div>
    <div v-else-if="loadDevicesError" class="dft-page-error">
      <el-icon class="dft-error-icon"><WarningFilled /></el-icon>
      <div class="dft-page-error-text">设备列表加载失败：{{ loadDevicesError }}</div>
      <el-button size="small" type="primary" @click="loadDevices">重试</el-button>
    </div>
    <empty-state v-else description="请先选择设备" :image-size="80" />
  </div>
</template>

<script setup lang="ts">
import { useDataFlowTrace } from '@/composables/data/useDataFlowTrace'

const {
  devices, points, selectedDevice, selectedPoint,
  device, pointInfo,
  readResult, readLatency, readError, isGood, reading,
  renderedPayload, renderError, rendering, copying, uplinkTemplate,
  sendResult, sending,
  loadingDevices, loadingPoints, loadDevicesError, loadPointsError,
  stepDone, stepStatus, latencyLevel,
  loadDevices, onDeviceChange, resetTrace, doRead, doRender, doSend, copyPayload,
} = useDataFlowTrace()
// stepDone/uplinkTemplate 供测试通过 setupState 访问
void stepDone
void uplinkTemplate
</script>

<style scoped src="./DataFlowTrace.css"></style>
