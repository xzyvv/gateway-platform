/**
 * useDevices 共享状态
 *
 * 把「所有子模块都要碰的状态」集中在这里一次性创建，
 * 再由各职责模块（列表加载 / 表单校验 / 增删改 / 连通性）按需取用。
 *
 * 之所以抽这一层：拆成多个文件后，form、devices、audit、route 等跨模块数据
 * 必须有唯一持有者，否则会出现两份 devices ref 之类的状态分裂。
 */
import { onUnmounted, reactive, ref } from 'vue'
import { useRoute } from 'vue-router'
import type { FormInstance } from 'element-plus'
import type { DeviceTemplate } from '@/api/templates'
import type { BusConfig, DeviceConfig } from '@/api/types'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import type { DeviceFormModel, DeviceState } from './types'

/**
 * 创建设备页面的共享状态
 *
 * 必须在组件 setup 上下文里调用（内部用到 useRoute / useDialogDirtyGuard）。
 */
export function createDeviceState(): DeviceState {
  const devices = ref<DeviceConfig[]>([])
  const loading = ref(false)
  /** 列表加载失败原因；空串表示无错误。失败时不能只显示「暂无设备」，否则会误判设备被删 */
  const loadError = ref('')
  const saving = ref(false)
  const testingConn = ref(false)
  const checking = ref(false)
  const showCreate = ref(false)
  const showDiscover = ref(false)
  const editing = ref(false)
  const formRef = ref<FormInstance>()

  // 设备表单：DeviceConfig 全部字段 + 扩展字段 group/tags
  const form: DeviceFormModel = reactive({
    id: '',
    name: '',
    template: '',
    bus: '',
    protocol: 'modbus' as DeviceConfig['protocol'],
    address: '',
    slave_id: null as number | null,
    address_map: null as { [key: string]: number } | null,
    // 采集周期覆盖（秒）：null = 使用模板默认周期（后端 Option<u64>）。
    // 不能下发 0——后端会当作「覆盖为 0 秒」，导致展开出的点位 interval=0 校验失败。
    interval: null as number | null,
    group: '',
    tags: [] as string[],
  })

  const templates = ref<DeviceTemplate[]>([])
  const templatesLoading = ref(false)
  const templateLoadError = ref('')

  const buses = ref<BusConfig[]>([])
  const busesLoading = ref(false)
  const busLoadError = ref('')

  const route = useRoute()

  // 未保存更改守卫：设备表单弹窗。弹窗级拦 X/ESC/取消；scope 桥接路由级注册表，
  // 弹窗开着跳路由 / 切 Hub tab 仍被 router.beforeEach / HubShell 拦截。
  const formGuard = useDialogDirtyGuard(
    showCreate,
    () => ({ ...form, tags: [...form.tags] }),
    { scope: route.path },
  )

  /** 组件卸载标记：卸载后不再写状态、不再弹提示 */
  let unmounted = false
  onUnmounted(() => { unmounted = true })

  return {
    devices, loading, loadError, saving, testingConn, checking,
    showCreate, showDiscover, editing, formRef, form,
    templates, templatesLoading, templateLoadError,
    buses, busesLoading, busLoadError,
    route, formGuard,
    isUnmounted: () => unmounted,
  }
}
