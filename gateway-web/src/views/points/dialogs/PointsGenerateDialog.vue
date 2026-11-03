<template>
  <el-dialog v-model="visible" title="批量生成点位" width="560px" @close="resetGenForm" :before-close="genGuard.beforeClose">
    <div class="batch-tip">
      <el-icon><InfoFilled /></el-icon>
      <span>按规则批量生成点位，将创建到当前选中设备「<strong>{{ deviceName }}</strong>」下。</span>
    </div>
    <el-form ref="genFormRef" :model="genForm" :rules="genRules" label-width="110px" scroll-to-error>
      <el-form-item label="名称前缀" prop="namePrefix">
        <el-input v-model="genForm.namePrefix" placeholder="如：电压" />
      </el-form-item>
      <el-form-item label="从站号" prop="slaveId">
        <el-input-number v-model="genForm.slaveId" :min="1" :max="247" controls-position="right" />
      </el-form-item>
      <el-form-item label="协议" prop="protocol">
        <el-select v-model="genForm.protocol" style="width: 100%">
          <el-option label="Modbus TCP" value="modbus-tcp" />
          <el-option label="Modbus RTU" value="modbus-rtu" />
          <el-option label="Modbus ASCII" value="modbus-ascii" />
        </el-select>
      </el-form-item>
      <el-form-item label="起始地址" prop="startAddr">
        <el-input-number v-model="genForm.startAddr" :min="0" :max="65535" controls-position="right" />
      </el-form-item>
      <el-form-item label="生成数量" prop="count">
        <el-input-number v-model="genForm.count" :min="1" :max="100" controls-position="right" />
      </el-form-item>
      <el-form-item label="地址步长" prop="step">
        <el-input-number v-model="genForm.step" :min="1" :max="10" controls-position="right" />
      </el-form-item>
      <el-form-item label="每个占寄存器数" prop="addressNum">
        <el-input-number v-model="genForm.addressNum" :min="1" :max="4" controls-position="right" />
      </el-form-item>
    </el-form>
    <div class="gen-preview">
      <strong>预览：</strong>
      <span v-for="(p, i) in genPreview" :key="i" class="gen-chip">{{ p }}</span>
    </div>
    <template #footer>
      <el-button @click="genGuard.close">取消</el-button>
      <el-button type="primary" :loading="generating" :disabled="generating" @click="doGenerate">生成 {{ genForm.count }} 个点位</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { reactive, computed, ref, getCurrentInstance } from 'vue'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { useRoute } from 'vue-router'
import { InfoFilled } from '@element-plus/icons-vue'
import { createPoint } from '@/api/points'
import type { DeviceConfig } from '@/api/types'
import { useDialogVisible } from '@/composables/core/useDialogVisible'
import { useUnmountedGuard } from '@/composables/core/useUnmountedGuard'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'

const props = defineProps<{
  modelValue: boolean
  deviceId: string
  deviceName: string
  devices: DeviceConfig[]
}>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  generated: []
}>()

const { isUnmounted } = useUnmountedGuard()

const visible = useDialogVisible(props, emit)

const genForm = reactive({
  namePrefix: '点位', slaveId: 1, protocol: 'modbus-rtu',
  startAddr: 0, count: 5, step: 1, addressNum: 2,
})
const genFormRef = ref<FormInstance>()
// 生成中标记：防止连点「生成」重复建点
const generating = ref(false)

// 字段级校验规则：错误落在对应输入框下方，现场人员一眼看到该改哪一栏
const genRules: FormRules = {
  namePrefix: [
    { required: true, message: '名称前缀不能为空', trigger: 'blur' },
    { max: 32, message: '名称前缀不能超过 32 个字符', trigger: 'blur' },
  ],
  slaveId: [{ type: 'number', min: 1, max: 247, message: '从站号需在 1-247 之间', trigger: 'blur' }],
  protocol: [{ required: true, message: '请选择协议', trigger: 'change' }],
  startAddr: [{ type: 'number', min: 0, max: 65535, message: '起始地址需在 0-65535 之间', trigger: 'blur' }],
  count: [{ type: 'number', min: 1, max: 100, message: '生成数量需在 1-100 之间', trigger: 'blur' }],
  step: [{ type: 'number', min: 1, max: 10, message: '地址步长需在 1-10 之间', trigger: 'blur' }],
  addressNum: [{ type: 'number', min: 1, max: 4, message: '每个占寄存器数需在 1-4 之间', trigger: 'blur' }],
}

// 弹窗未保存修改守卫：X/ESC/取消关闭前对比基线，有改动先确认
// 路由级脏标记桥接：只有挂了 router 的运行环境才取路由，取不到就只启用弹窗级守卫
const dirtyScope = getCurrentInstance()?.appContext.config.globalProperties.$router ? useRoute().path : undefined
const genGuard = useDialogDirtyGuard(visible, () => genForm, dirtyScope ? { scope: dirtyScope } : {})
const genPreview = computed(() => {
  const n = Math.min(genForm.count, 5)
  const arr: string[] = []
  for (let i = 0; i < n; i++) {
    const addr = genForm.startAddr + i * genForm.step
    arr.push(genForm.namePrefix + (i + 1) + '@' + addr)
  }
  arr.push('共 ' + genForm.count + ' 个')
  return arr
})
function resetGenForm() {
  genForm.namePrefix = '点位'; genForm.slaveId = 1; genForm.protocol = 'modbus-rtu'
  genForm.startAddr = 0; genForm.count = 5; genForm.step = 1; genForm.addressNum = 2
}
async function doGenerate() {
  if (generating.value) return
  if (!props.deviceId) { ElMessage.warning('请先选择设备'); return }
  // 字段级校验：错误已落在对应输入框下方（scroll-to-error 自动定位首个错误项），不通过就不生成
  const formEl = genFormRef.value
  if (formEl) {
    const ok = await formEl.validate().catch(() => false)
    if (!ok) return
  }
  generating.value = true
  try {
    const bus = props.devices.find((d) => d.id === props.deviceId)?.bus || ''
    const CONCURRENCY = 6
    let ok = 0
    let fail = 0
    const queue: Array<{ addr: number; idx: number }> = []
    for (let i = 0; i < genForm.count; i++) {
      queue.push({ addr: genForm.startAddr + i * genForm.step, idx: i + 1 })
    }
    async function worker() {
      while (!isUnmounted()) {
        const item = queue.shift()
        if (!item) break
        try {
          await createPoint(props.deviceId, {
            id: genForm.namePrefix.toLowerCase() + '_' + item.addr,
            name: genForm.namePrefix + item.idx,
            bus,
            protocol: genForm.protocol,
            address: String(item.addr),
            slave_id: genForm.slaveId,
            address_num: genForm.addressNum,
            device_id: props.deviceId,
            // 必填字段补默认值（与 PointsBatchImport 一致）
            function_code: null,
            data_type: null,
            byte_order: null,
            register_count: null,
            data_id: null,
            interval: 1,
            scale: 1,
            offset: 0,
            formula: null,
            ct_ratio: null,
            pt_ratio: null,
            deadband: 0,
            on_change: false,
            unit: null,
            alarm_high: null,
            alarm_low: null,
            aggregation: null,
            modbus_register: null,
          })
          ok++
        } catch { fail++ }
      }
    }
    await Promise.all(
      Array.from({ length: Math.min(CONCURRENCY, genForm.count) }, () => worker()),
    )
    if (isUnmounted()) return
    ElMessage.success('生成完成：成功 ' + ok + '，失败 ' + fail)
    visible.value = false
    emit('generated')
  } finally {
    if (!isUnmounted()) generating.value = false
  }
}
</script>

<style scoped>
.batch-tip { display: flex; align-items: center; gap: 6px; padding: 8px 12px; background: var(--color-primary-bg); border-radius: var(--radius-small); margin-bottom: 16px; color: var(--color-primary); }
.gen-preview { margin-top: 12px; padding: 8px; background: var(--bg-fill); border-radius: var(--radius-small); }
.gen-chip { display: inline-block; margin: 2px 4px; padding: 2px 8px; background: var(--bg-card); border: 1px solid var(--border-lighter); border-radius: var(--radius-card); font-size: 0.75rem; }
</style>
