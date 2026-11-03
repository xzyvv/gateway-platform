<template>
  <!--
    E1：调试工具独占总线时的「暂停采集」引导。
    原先用户只会收到「请先暂停该总线的采集后再试」这条做不到的指引；
    本组件把它变成一次点击，并在暂停后提示「调试完记得恢复」。
  -->
  <el-alert
    v-if="(visible || paused) && bus"
    :type="paused ? 'warning' : 'info'"
    :closable="false"
    show-icon
    class="bus-pause-banner"
  >
    <template #title>
      <span v-if="paused">
        「{{ bus }}」的采集已暂停，可继续调试；调试结束后请点击「恢复采集」，否则该总线不再采数。
      </span>
      <span v-else>「{{ bus }}」正被采集引擎占用，暂停该总线的采集即可独占调试。</span>
    </template>
    <div class="actions">
      <el-button
        v-if="!paused"
        size="small"
        type="warning"
        :loading="busy"
        @click="pause"
      >
        暂停该总线采集
      </el-button>
      <el-button
        v-else
        size="small"
        type="success"
        :loading="busy"
        @click="resume"
      >
        恢复采集
      </el-button>
    </div>
  </el-alert>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { listPausedBuses, pauseBusCollect, resumeBusCollect } from '@/api/buses'
import { extractErrorMessage } from '@/utils/errorHandler'

const props = defineProps<{
  /** 目标总线名（即 port_name） */
  bus: string
  /** 是否展示（通常由「总线被采集占用」的错误判定驱动） */
  visible: boolean
}>()
const emit = defineEmits<{ (e: 'paused'): void; (e: 'resumed'): void }>()

const paused = ref(false)
const busy = ref(false)

/** 打开时同步真实状态：用户可能已在本页或其他页暂停过，刷新后仍应看到「已暂停」 */
async function syncPaused() {
  if (!props.visible) return
  try {
    const r = await listPausedBuses()
    paused.value = r.paused_buses.includes(props.bus)
  } catch {
    // 状态查询失败不阻断暂停/恢复按钮，按未暂停呈现
    paused.value = false
  }
}
watch(() => [props.visible, props.bus], () => { void syncPaused() }, { immediate: true })

async function pause() {
  busy.value = true
  try {
    const r = await pauseBusCollect(props.bus)
    paused.value = r.paused_buses.includes(props.bus)
    ElMessage.success(`已暂停「${props.bus}」的采集，现在可以独占调试`)
    emit('paused')
  } catch (e) {
    ElMessage.error(`暂停失败：${extractErrorMessage(e, '未知错误')}`)
  } finally {
    busy.value = false
  }
}

async function resume() {
  busy.value = true
  try {
    const r = await resumeBusCollect(props.bus)
    paused.value = r.paused_buses.includes(props.bus)
    ElMessage.success(`已恢复「${props.bus}」的采集`)
    emit('resumed')
  } catch (e) {
    ElMessage.error(`恢复失败：${extractErrorMessage(e, '未知错误')}`)
  } finally {
    busy.value = false
  }
}
</script>

<style scoped>
.bus-pause-banner {
  margin-bottom: 12px;
}
.actions {
  margin-top: 8px;
}
</style>
