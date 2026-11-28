<script setup lang="ts">
/**
 * 设备资产信息弹窗（A7）。
 *
 * 从 `DeviceList.vue` 拆出：资产表单与设备列表是两块不相干的功能——列表是
 * 「读 + 跳」，资产是「单设备的一条台账记录」，共用一个组件文件会让两者互相干扰。
 *
 * 行为保持不变：
 *   - 打开时先拉该设备的资产视图；接口失败/未登记 → 按「新增」处理（空表单）
 *   - 保存走 upsertAssetView，成功后关闭并提示
 */
import { computed, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { getAssetView, upsertAssetView, ALL_TAGS, type DeviceAssetView } from '@/api/acquisition/deviceOps'
import { extractErrorMessage } from '@/utils/error/errorHandler'

const props = defineProps<{
  /** 弹窗可见性 */
  modelValue: boolean
  /** 当前操作的设备 ID（空串表示未选中） */
  deviceId: string
  /** 是否具备写权限（device.write） */
  canWrite: boolean
}>()

const emit = defineEmits<{ (e: 'update:modelValue', v: boolean): void }>()

const loading = ref(false)
const saving = ref(false)
const cur = ref<DeviceAssetView | null>(null)

const title = computed(() => (cur.value ? `资产信息 · ${cur.value.deviceId}` : '资产信息'))

/** 空表单模板：接口返回空 / 失败时按新增处理 */
function emptyAsset(deviceId: string): DeviceAssetView {
  return {
    deviceId,
    assetNo: '',
    vendor: '',
    model: '',
    location: '',
    owner: '',
    purchasedAt: '',
    calibratedAt: '',
    tags: [],
  }
}

/** 弹窗打开且设备切换时重新加载资产视图 */
watch(
  () => [props.modelValue, props.deviceId] as const,
  async ([visible, deviceId]) => {
    if (!visible || !deviceId) return
    loading.value = true
    cur.value = null
    try {
      cur.value = await getAssetView(deviceId)
    } catch {
      cur.value = emptyAsset(deviceId)
    } finally {
      loading.value = false
    }
  },
  { immediate: true },
)

async function save() {
  const current = cur.value
  if (!current || saving.value) return
  saving.value = true
  try {
    await upsertAssetView({
      device_id: current.deviceId,
      asset_no: current.assetNo,
      vendor: current.vendor,
      model: current.model,
      location: current.location,
      owner: current.owner,
      purchased_at: current.purchasedAt,
      calibrated_at: current.calibratedAt,
      tags: current.tags,
    })
    ElMessage.success('资产信息已保存')
    emit('update:modelValue', false)
  } catch (e) {
    ElMessage.error(extractErrorMessage(e))
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <el-dialog
    :model-value="modelValue"
    :title="title"
    width="560px"
    :close-on-click-modal="false"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <el-skeleton v-if="loading" :rows="5" animated />
    <el-form v-else-if="cur" label-width="90px">
      <el-form-item label="设备 ID"><el-input :model-value="cur.deviceId" disabled /></el-form-item>
      <el-form-item label="资产编号"><el-input v-model="cur.assetNo" placeholder="请输入资产编号" /></el-form-item>
      <el-form-item label="厂家"><el-input v-model="cur.vendor" placeholder="请输入厂家" /></el-form-item>
      <el-form-item label="型号"><el-input v-model="cur.model" placeholder="请输入型号" /></el-form-item>
      <el-form-item label="安装位置"><el-input v-model="cur.location" placeholder="请输入安装位置" /></el-form-item>
      <el-form-item label="负责人"><el-input v-model="cur.owner" placeholder="请输入负责人" /></el-form-item>
      <el-form-item label="采购日期">
        <el-date-picker v-model="cur.purchasedAt" type="date" value-format="YYYY-MM-DD" placeholder="选择日期" />
      </el-form-item>
      <el-form-item label="校准日期">
        <el-date-picker v-model="cur.calibratedAt" type="date" value-format="YYYY-MM-DD" placeholder="选择日期" />
      </el-form-item>
      <el-form-item label="标签">
        <el-select v-model="cur.tags" multiple filterable placeholder="选择标签">
          <el-option v-for="t in ALL_TAGS" :key="t" :label="t" :value="t" />
        </el-select>
      </el-form-item>
    </el-form>
    <template #footer>
      <el-button @click="emit('update:modelValue', false)">取消</el-button>
      <el-button type="primary" :loading="saving" :disabled="!canWrite" @click="save">保存</el-button>
    </template>
  </el-dialog>
</template>
