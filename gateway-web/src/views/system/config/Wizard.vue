<template>
  <div class="wz">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="部署向导" icon="MagicStick">
          <template #extra>
            <el-button :icon="Refresh" :loading="reloading" @click="reloadAll">重新读取网关状态</el-button>
          </template>
        </SectionHeader>
      </template>
      <el-alert
        type="warning"
        :closable="false"
        show-icon
        title="本向导的每一步都会真实写入网关配置"
        description="总线写入配置并热重载；设备、点位、上行转发各步成功后立即生效。中途退出时已完成的部分会保留，可到「设备清单」「点位管理」「运行参数」继续修改或删除。"
      />
    </el-card>

    <el-steps :active="step" align-center finish-status="success" style="margin-bottom: 24px">
      <el-step v-for="s in STEPS" :key="s.title" :title="s.title" :description="s.desc" />
    </el-steps>

    <el-card shadow="never" class="section-card">
      <template #header>
        <strong>步骤 {{ step + 1 }}: {{ STEPS[step]?.title }}</strong>
      </template>

      <!-- 各步骤的渲染与局部交互都在子组件里；状态与流程编排留在本组件 -->
      <div class="step-content">
        <WizardStepBus
          v-if="step === 0"
          v-model:bus-pick="busPick"
          v-model:show-new-bus="showNewBus"
          v-model:new-bus="newBus"
          :buses="buses"
          :adding-bus="addingBus"
          @add-bus="addBus"
        />
        <WizardStepDevice
          v-else-if="step === 1"
          v-model:device-form="deviceForm"
          :selected-bus="selectedBus"
          :templates="templates"
          @open-new-template="openNewTemplate"
        />
        <WizardStepPoints
          v-else-if="step === 2"
          v-model:manual="manual"
          :device-name="deviceForm.name"
          :selected-bus="selectedBus"
          :device-address="createdDevice?.address"
          :points="points"
          :loading-points="loadingPoints"
          :adding-point="addingPoint"
          @add-point="handleAddPoint"
        />
        <WizardStepUplink
          v-else-if="step === 3"
          v-model:uplink-form="uplinkForm"
        />
        <WizardStepVerify
          v-else
          :device-name="deviceForm.name"
          :point-count="points.length"
          :uplink-enabled="uplinkForm.enabled"
          :uplink-topic="uplinkForm.topic"
          :verifying="verifying"
          :verify-summary="verifySummary"
          :verify-failed="verifyFailed"
          :read-results="readResults"
          @verify="handleVerify"
        />
      </div>

      <div class="step-actions">
        <el-button @click="prev" :disabled="step === 0 || busy">上一步</el-button>
        <el-button v-if="step < STEPS.length - 1" type="primary" :loading="busy" @click="next">下一步</el-button>
        <el-button v-else type="success" @click="finish" :disabled="!verified">完成</el-button>
      </div>
    </el-card>

    <!-- 就地新建模板 -->
    <WizardNewTemplateDialog
      v-model="showNewTemplate"
      v-model:form="newTemplate"
      :saving="savingTemplate"
      @save="saveNewTemplate"
    />
  </div>
</template>

<script setup lang="ts">
/**
 * 部署向导（真实写入网关）
 *
 * 旧实现是一个纯前端演示：步骤清单来自 `@/mock/common/wizard`，表单值从不提交，
 * 第 5 步「采集测试」只是 `setTimeout(1500)` 后弹成功——用户以为配好了，网关其实一无所知。
 *
 * 现按真实链路串起来，每步都调用后端并立即生效：
 *  1. 总线：`GET /api/config` + `POST /api/config`（追加一条总线并热重载）
 *  2. 设备：`POST /api/devices`（模板必选）
 *  3. 点位：`POST /api/devices/:id/points`，列表用 `GET /api/devices/:id/points`
 *  4. 上行：`POST /api/config`（写 `mqtt` 分节：broker/topic/周期/QoS）
 *  5. 验证：逐点 `GET /api/points/:id/read`，失败项如实列出
 *
 * 关于「回滚」：每步是独立的真实写入，放弃向导不撤销已完成步骤（避免误删现场刚配好的
 * 设备）。失败时停在当前步并提示已生效的部分，用户可到对应页面修改或删除。
 *
 * 结构：5 个步骤各拆一个子组件（见 `./wizard/`），每步的状态与真实接口调用
 * 收在对应的 composable 里（useBusStep / useDeviceStep / usePointStep /
 * useUplinkStep / useVerifyStep）；本组件只保留步骤状态机与流程编排，
 * 子组件通过 v-model / props / emits 与本组件通信。
 */
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { Refresh } from '@element-plus/icons-vue'
import SectionHeader from '@/components/SectionHeader.vue'
import WizardStepBus from './wizard/WizardStepBus.vue'
import WizardStepDevice from './wizard/WizardStepDevice.vue'
import WizardStepPoints from './wizard/WizardStepPoints.vue'
import WizardStepUplink from './wizard/WizardStepUplink.vue'
import WizardStepVerify from './wizard/WizardStepVerify.vue'
import WizardNewTemplateDialog from './wizard/WizardNewTemplateDialog.vue'
import { useBusStep } from './wizard/useBusStep'
import { useDeviceStep } from './wizard/useDeviceStep'
import { usePointStep } from './wizard/usePointStep'
import { useUplinkStep } from './wizard/useUplinkStep'
import { useVerifyStep } from './wizard/useVerifyStep'
import type { WizardStepMeta } from './wizard/types'

const router = useRouter()

const STEPS: WizardStepMeta[] = [
  { title: '配置通信总线', desc: '选择或新建一条总线' },
  { title: '新增设备', desc: '名称、从站地址与设备模板' },
  { title: '配置点位', desc: '为设备添加至少一个点位' },
  { title: '配置上行转发', desc: 'MQTT 主通道地址与主题' },
  { title: '验证采集', desc: '逐点读取，确认数据可用' },
]

const step = ref(0)
const busy = ref(false)
const reloading = ref(false)

// 第 1 步：总线
const { buses, busPick, showNewBus, addingBus, newBus, selectedBus, loadBuses, addBus } = useBusStep()
// 第 2 步：设备与模板
const { templates, deviceForm, createdDevice, showNewTemplate, savingTemplate, newTemplate, loadTemplates, createDev, openNewTemplate, saveNewTemplate } =
  useDeviceStep()
// 第 3 步：点位
const { points, loadingPoints, manual, addingPoint, loadPoints, addPoint } = usePointStep()
// 第 4 步：上行转发
const { uplinkForm, loadUplink, saveUplink } = useUplinkStep()
// 第 5 步：验证采集
const { verifying, verified, verifySummary, verifyFailed, readResults, verify } = useVerifyStep()

async function reloadAll() {
  reloading.value = true
  try {
    await Promise.all([loadBuses(), loadTemplates(), loadUplink()])
    ElMessage.success('已重新读取网关状态')
  } finally {
    reloading.value = false
  }
}

async function next() {
  if (step.value === 0) {
    if (!selectedBus.value) {
      ElMessage.warning(showNewBus.value ? '请先完成新增总线' : '请选择一条总线')
      return
    }
    step.value = 1
    return
  }
  if (step.value === 1) {
    if (!selectedBus.value) {
      ElMessage.warning('请先选择总线')
      step.value = 0
      return
    }
    busy.value = true
    const ok = await createDev(selectedBus.value)
    if (ok) await loadPoints(createdDevice.value?.id)
    busy.value = false
    if (ok) step.value = 2
    return
  }
  if (step.value === 2) {
    if (!points.value.length) {
      ElMessage.warning('请至少添加一个点位，否则无法验证采集')
      return
    }
    step.value = 3
    return
  }
  if (step.value === 3) {
    busy.value = true
    const ok = await saveUplink()
    busy.value = false
    if (ok) step.value = 4
  }
}

function prev() {
  if (step.value > 0) step.value--
}

/** 第 3 步：补录点位（设备、总线与从站地址取自主组件状态） */
function handleAddPoint() {
  return addPoint(createdDevice.value, selectedBus.value, deviceForm.value.slaveId)
}

/** 第 5 步：执行采集测试 */
function handleVerify() {
  return verify(points.value)
}

function finish() {
  ElMessage.success('部署向导完成：总线、设备、点位与上行通道均已写入网关')
  router.push('/devices?tab=devices')
}

onMounted(() => {
  loadBuses()
  loadTemplates()
  loadUplink()
})
</script>

<style scoped>
.wz { padding: 16px; max-width: 1100px; }
.section-card { margin-bottom: 16px; }
.step-content { padding: 12px 0; }
.step-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; padding-top: 16px; border-top: 1px solid var(--el-border-color-lighter); }
</style>
