<template>
  <el-dialog v-model="visible" title="设备扫描发现" width="720px" @close="resetDiscover" :before-close="discoverGuard.beforeClose">
    <div class="discover-form">
      <el-form inline label-width="70px" ref="formRef" :model="discover" :rules="discoverRules" scroll-to-error>
        <el-form-item label="总线" prop="bus">
          <el-input v-model="discover.bus" placeholder="如 bus1" style="width: 160px" />
        </el-form-item>
        <el-form-item label="协议" prop="protocol">
          <el-select v-model="discover.protocol" style="width: 140px">
            <el-option v-for="opt in protocolOptions" :key="opt.value" :label="opt.label" :value="opt.value" />
          </el-select>
        </el-form-item>
        <el-form-item label="起始地址" prop="start">
          <el-input-number v-model="discover.start" :min="1" :max="247" controls-position="right" />
        </el-form-item>
        <el-form-item label="结束地址" prop="end">
          <el-input-number v-model="discover.end" :min="1" :max="247" controls-position="right" />
        </el-form-item>
        <el-form-item>
          <el-button type="primary" :loading="discovering" :disabled="discovering" @click="doDiscover">
            <el-icon><Search /></el-icon> 开始扫描
          </el-button>
        </el-form-item>
      </el-form>
      <div class="discover-tip">
        <el-icon><InfoFilled /></el-icon>
        <span>
          扫描总线 <strong>{{ discover.bus || '?' }}</strong> 上从地址 {{ discover.start }} 到 {{ discover.end }} 的设备，可能需要数秒至数十秒。
          <template v-if="!discover.bus">请先在上方填写总线名。</template>
        </span>
      </div>
    </div>

    <DataTable
      :data="discoverResults"
      :columns="discoverColumns"
      row-key="address"
      :loading="discovering"
      :max-height="360"
      stripe
      empty-text="暂无扫描结果"
    />
  </el-dialog>
</template>

<script setup lang="ts">
import { ref, reactive, h, getCurrentInstance } from 'vue'
import { ElMessage, ElButton, type FormInstance, type FormRules } from 'element-plus'
import { useRoute } from 'vue-router'
import { Search, InfoFilled } from '@element-plus/icons-vue'
import { scanBus, respondedSlaves } from '@/api/protocolTools/slaveScan'
import { isExistingDevice } from '@/utils/deviceCopy'
import { setFormFieldErrors, clearFormFieldErrors } from '@/utils/formFieldError'
import { useDialogVisible } from '@/composables/core/useDialogVisible'
import { useUnmountedGuard } from '@/composables/core/useUnmountedGuard'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import type { DeviceConfig } from '@/api/types'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'

interface Props {
  modelValue: boolean
  protocolOptions: { value: string; label: string }[]
  devices: DeviceConfig[]
}
const props = defineProps<Props>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  add: [row: Record<string, unknown>, bus: string, protocol: string]
}>()

const visible = useDialogVisible(props, emit)
const { isUnmounted } = useUnmountedGuard()

const discovering = ref(false)
const discover = reactive({ bus: '', protocol: 'modbus', start: 1, end: 247 })
const discoverResults = ref<Record<string, unknown>[]>([])
const formRef = ref<FormInstance>()

// 字段级校验规则：错误落在对应输入框下方，现场人员一眼看到该改哪一栏
const discoverRules: FormRules = {
  bus: [
    { required: true, message: '请填写总线名', trigger: 'blur' },
    { max: 32, message: '总线名不能超过 32 个字符', trigger: 'blur' },
  ],
  protocol: [{ required: true, message: '请选择协议', trigger: 'change' }],
  start: [{ type: 'number', min: 1, max: 247, message: '起始地址需在 1-247 之间', trigger: 'blur' }],
  end: [{ type: 'number', min: 1, max: 247, message: '结束地址需在 1-247 之间', trigger: 'blur' }],
}

// 弹窗未保存修改守卫：改了总线/协议/地址范围后直接关闭会静默丢失，先确认
// 路由级脏标记桥接：只有挂了 router 的运行环境才取路由，取不到就只启用弹窗级守卫
const dirtyScope = getCurrentInstance()?.appContext.config.globalProperties.$router ? useRoute().path : undefined
const discoverGuard = useDialogDirtyGuard(visible, () => discover, dirtyScope ? { scope: dirtyScope } : {})

// DataTable 列定义：宽度/固定列与原列保持一致
const discoverColumns: ColumnDef<Record<string, unknown>>[] = [
  { key: 'address', title: '地址', width: 120 },
  {
    key: 'device_id',
    title: '设备 ID',
    minWidth: 160,
    ellipsis: true,
    cell: (row) => h('span', String(row.device_id || '—')),
  },
  {
    key: 'protocol',
    title: '协议',
    width: 100,
    cell: (row) => h('span', String(row.protocol || discover.protocol)),
  },
  {
    key: 'extra',
    title: '附加信息',
    minWidth: 160,
    ellipsis: true,
    cell: (row) => {
      const extra = row.extra
      if (typeof extra === 'string') return h('span', extra)
      if (extra) return h('span', JSON.stringify(extra))
      return h('span', '—')
    },
  },
  actionColumn<Record<string, unknown>>({
    key: 'action',
    width: 140,
    fixed: 'right',
    cell: (row) =>
      h(
        ElButton,
        {
          size: 'small',
          type: 'primary',
          link: true,
          disabled: isExistingDeviceWrapper(row),
          onClick: () => addFromDiscover(row),
        },
        () => (isExistingDeviceWrapper(row) ? '已存在' : '新增设备'),
      ),
  }),
]

function resetDiscover() {
  discoverResults.value = []
  clearFormFieldErrors(formRef.value!)
}

function isExistingDeviceWrapper(row: Record<string, unknown>) {
  return isExistingDevice(row, props.devices, discover.bus)
}

async function doDiscover() {
  // 字段级校验：错误已落在对应输入框下方（scroll-to-error 自动定位首个错误项），不通过就不扫描
  const formEl = formRef.value
  if (formEl) {
    const ok = await formEl.validate().catch(() => false)
    if (!ok) return
  }
  // ROI: 跨字段校验同时高亮涉及的字段（错误显示在字段下方，不再只弹 toast）
  if (discover.start > discover.end) {
    setFormFieldErrors(formRef.value!, {
      start: '不能大于结束地址',
      end: '不能小于起始地址',
    })
    return
  }
  clearFormFieldErrors(formRef.value!, ['start', 'end'])
  discovering.value = true
  discoverResults.value = []
  try {
    // P0-2：与「从站扫描」页共用同一个扫描端点实现
    const results = await scanBus(discover.bus, discover.start, discover.end)
    if (isUnmounted()) return
    const found = respondedSlaves(results)
    discoverResults.value = found.map((r) => ({
      address: String(r.slaveId),
      device_id: r.deviceName || `slave_${r.slaveId}`,
      protocol: discover.protocol,
      extra: { func_code: r.funcCode ?? null, latency_ms: r.latencyMs ?? null },
    }))
  } catch {
    // POST 扫描：全局拦截器只留痕不弹窗，需在此给出动作语境提示
    ElMessage.error('设备扫描失败，请检查总线名与地址范围是否可用')
  } finally {
    if (!isUnmounted()) discovering.value = false
  }
}

function addFromDiscover(row: Record<string, unknown>) {
  emit('add', row, discover.bus, String(row.protocol ?? discover.protocol) || 'modbus')
  visible.value = false
}
</script>

<style scoped>
.discover-form { margin-bottom: 12px; }
.discover-tip { display: flex; align-items: flex-start; gap: 6px; color: var(--text-secondary); font-size: 0.8125rem; margin-top: 8px; }
</style>
