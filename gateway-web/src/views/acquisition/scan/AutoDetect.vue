<template>
  <div class="ad"><SectionHeader title="参数探测" icon="MagicStick" />
    <el-alert
      class="section-card"
      type="info"
      show-icon
      :closable="false"
      title="用途：确认所选设备（已知地址）的通信参数"
      description="逐项探测波特率、数据位、校验位、停止位等参数；探测前该设备的从站地址必须已知。若不知道地址，先用「从站扫描」找出总线上有哪些从站。"
    />
    <el-card shadow="never" class="section-card">
      <el-form inline>
        <el-form-item label="目标设备">
          <el-select v-model="deviceId" style="width:220px" :loading="targetsLoading" placeholder="选择设备" aria-label="目标设备" no-data-text="暂无设备，请先在设备管理中添加">
            <el-option v-for="o in deviceOptions" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
        </el-form-item>
        <el-form-item>
          <el-button type="primary" @click="detect" :loading="detecting" :disabled="!autoDetectSupported" :icon="MagicStick">开始探测</el-button>
          <el-button v-if="detecting" @click="stopDetect">停止</el-button>
        </el-form-item>
      </el-form>
      <el-alert
        v-if="capsFailed"
        type="warning"
        show-icon
        :closable="false"
        title="协议能力元数据加载失败"
        description="暂时无法判断当前设备是否支持参数探测，按钮已禁用；请刷新页面重试。"
      />
      <el-alert
        v-else-if="capsLoaded && selectedProtocol && !autoDetectSupported"
        type="info"
        show-icon
        :closable="false"
        :title="`当前设备协议（${selectedProtocol}）不支持参数探测`"
        description="该协议不依赖逐项试探通信参数，参数探测不适用（不是尚未实现）；如需探测通信参数，请切换到 Modbus 等支持参数探测的设备。"
      />
      <el-progress v-if="detecting" :percentage="progress" :stroke-width="8" style="margin-top:12px" />
      <AutoDetectSteps :steps="steps" />

      <AutoDetectSummary
        v-if="done"
        :success-steps="successSteps"
        :failed-count="failedSteps.length"
        @copy="copyParams"
        @apply="gotoCommParams"
      />
    </el-card>
  </div>
</template>

<script setup lang="ts">
/**
 * 参数探测（薄编排层）
 *
 * 探测状态机（启动/中止/轮询/汇总/复制）在 composables/acquisition/useAutoDetect.ts；
 * 展示按区块拆到同目录 parts/ 下：
 * - parts/AutoDetectSteps.vue   逐项探测时间线（含失败原因与诊断建议）
 * - parts/AutoDetectSummary.vue 成功参数汇总、复制与跳转应用入口
 *
 * 本文件只保留布局、参数表单与事件接线。
 */
import { useRouter } from 'vue-router'
import { MagicStick } from '@element-plus/icons-vue'
import { useAutoDetect } from '@/composables/acquisition/useAutoDetect'
import AutoDetectSteps from './parts/AutoDetectSteps.vue'
import AutoDetectSummary from './parts/AutoDetectSummary.vue'

const router = useRouter()
const {
  deviceOptions,
  targetsLoading,
  deviceId,
  capsLoaded,
  capsFailed,
  selectedProtocol,
  autoDetectSupported,
  detecting,
  done,
  steps,
  progress,
  successSteps,
  failedSteps,
  detect,
  stopDetect,
  copyParams,
} = useAutoDetect()

/** 应用入口：通信参数页按设备编辑超时/字节序等，跳到对应 tab 由用户确认后写入配置 */
function gotoCommParams() {
  router.push({ path: '/acquisition/comm-tuning', query: { tab: 'comm-params' } })
}
</script>

<style scoped>
.ad{padding:16px}.section-card{margin-bottom:16px}
</style>
