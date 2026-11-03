<template>
  <el-dialog v-model="visible" title="复制点位到其他设备" width="560px" @close="onCopyDialogClose" :before-close="copyGuard.beforeClose">
    <el-form label-width="100px">
      <el-form-item label="源设备">
        <el-select v-model="copySource" placeholder="选择源设备" style="width: 100%">
          <el-option
            v-for="d in devices.filter(d => d.id !== selectedDevice)"
            :key="d.id"
            :label="d.name"
            :value="d.id"
          />
        </el-select>
      </el-form-item>
      <el-form-item label="目标设备">
        <el-input :value="selectedDeviceName" disabled />
      </el-form-item>
      <el-form-item label="复制策略">
        <el-radio-group v-model="copyMode">
          <el-radio value="skip">跳过同名点位</el-radio>
          <el-radio value="overwrite">覆盖同名点位</el-radio>
        </el-radio-group>
      </el-form-item>
    </el-form>
    <template #footer>
      <el-button @click="copyGuard.close">取消</el-button>
      <el-button type="primary" :loading="copying" :disabled="copying || !copySource" @click="doCopy">
        开始复制
      </el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { ElMessage } from 'element-plus'
import { listPoints, createPoint } from '@/api/points'
import type { DeviceConfig, PointConfig } from '@/api/types'
import { filterPointsForCopy, mapPointToTarget } from '@/utils/deviceCopy'
import { useDialogVisible } from '@/composables/core/useDialogVisible'
import { useUnmountedGuard } from '@/composables/core/useUnmountedGuard'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'

const props = defineProps<{
  modelValue: boolean
  devices: DeviceConfig[]
  selectedDevice: string
  selectedDeviceName: string
  points: PointConfig[]
}>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  copied: []
}>()

const { isUnmounted } = useUnmountedGuard()

const visible = useDialogVisible(props, emit)

const copySource = ref('')
const copyMode = ref<'skip' | 'overwrite'>('skip')
const copying = ref(false)

// 弹窗未保存修改守卫：X/ESC/取消关闭前对比基线（源设备/复制策略），有改动先确认
const copyGuard = useDialogDirtyGuard(visible, () => ({ copySource: copySource.value, copyMode: copyMode.value }))

async function doCopy() {
  if (!copySource.value || !props.selectedDevice) return
  copying.value = true
  try {
    const sourcePoints = await listPoints(copySource.value)
    if (isUnmounted()) return
    if (!sourcePoints.length) {
      ElMessage.warning('源设备暂无点位')
      return
    }
    const { toCreate, skipped } = filterPointsForCopy(sourcePoints, props.points, copyMode.value)
    let created = 0
    /** 单个点位创建失败数：写操作失败全局静默，必须在这里汇总告知用户 */
    let failed = 0
    const CONCURRENCY = 6
    const queue = [...toCreate]
    async function worker() {
      while (!isUnmounted()) {
        const p = queue.shift()
        if (!p) break
        const newPoint = mapPointToTarget(p, props.selectedDevice)
        try {
          await createPoint(props.selectedDevice, newPoint)
          created++
        } catch { failed++ }
      }
    }
    await Promise.all(
      Array.from({ length: Math.min(CONCURRENCY, toCreate.length) }, () => worker()),
    )
    if (isUnmounted()) return
    // 有失败点时降级为 warning 并显式给出失败数，避免"成功 0 个"被当成正常
    const summary = '复制完成：成功 ' + created + ' 个'
      + (failed ? '，失败 ' + failed + ' 个' : '')
      + (skipped ? '，跳过 ' + skipped + ' 个' : '')
    if (failed) ElMessage.warning(summary)
    else ElMessage.success(summary)
    visible.value = false
    copySource.value = ''
    emit('copied')
  } catch {
    // 源设备点位加载或复制流程失败：GET 由全局提示，写操作需在此给出动作语境
    ElMessage.error('复制点位失败，请检查源设备与目标设备状态')
  } finally {
    if (!isUnmounted()) copying.value = false
  }
}

function onCopyDialogClose() {
  copySource.value = ''
}
</script>
