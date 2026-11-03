/**
 * 部署向导 · 第 1 步（通信总线）的状态与真实接口调用。
 *
 * 拆出本 composable 的原因：总线列表的读取、新建总线的写入都是真实网关操作，
 * 与「渲染一个下拉框」耦合很重，留在页面里会让页面变成巨型文件。
 * 这里持有总线相关的全部响应式状态，并把状态与调用方（主组件）共享，
 * 主组件只负责把状态通过 v-model 传给子组件渲染。
 *
 * 典型应用场景：部署向导的第 1 步（配置通信总线）。
 */
import { ref, computed } from 'vue'
import { ElMessage } from 'element-plus'
import { getConfig, postConfig } from '@/api/config'
import { protocolDefaultTimeoutMs } from '@/composables/devices/useProtocol'
import type { BusConfig } from '@/api/types'
import type { NewBusForm } from './types'

export function useBusStep() {
  /** 网关现有总线列表 */
  const buses = ref<BusConfig[]>([])
  /** 当前选中的总线端口名 */
  const busPick = ref('')
  /** 是否展开「新建总线」表单 */
  const showNewBus = ref(false)
  /** 新建总线请求进行中 */
  const addingBus = ref(false)
  /** 新建总线的表单值（三个字段，最终组装成 BusConfig） */
  const newBus = ref<NewBusForm>({ port_name: '', baud_rate: 9600, protocol: 'modbus' })

  /** 当前选中的总线对象 */
  const selectedBus = computed(() => buses.value.find((b) => b.port_name === busPick.value))

  /** 读取网关总线配置；失败只提示不阻断 */
  async function loadBuses() {
    try {
      const cfg = await getConfig()
      buses.value = Array.isArray(cfg.buses) ? cfg.buses : []
      if (!busPick.value && buses.value[0]) busPick.value = buses.value[0].port_name
    } catch {
      ElMessage.error('加载总线配置失败')
    }
  }

  /** 新建一条总线并写入配置（追加后热重载），成功后自动选中 */
  async function addBus() {
    const port = newBus.value.port_name.trim()
    if (!port) {
      ElMessage.warning('请先选择或输入串口端口')
      return
    }
    if (buses.value.some((b) => b.port_name === port)) {
      ElMessage.warning(`端口 ${port} 已存在，直接在上方选择该总线即可`)
      return
    }
    addingBus.value = true
    try {
      const cfg = await getConfig()
      const list: BusConfig[] = Array.isArray(cfg.buses) ? [...cfg.buses] : []
      list.push({
        port_name: port,
        baud_rate: newBus.value.baud_rate,
        parity: 'none',
        data_bits: 8,
        stop_bits: 1,
        timeout_ms: protocolDefaultTimeoutMs(newBus.value.protocol),
        protocol: newBus.value.protocol,
        retry_count: 2,
        retry_delay_ms: 200,
        de_pin: null,
        modbus_mode: 'rtu',
        dlt645_version: '2007',
        variant: null,
        asdu_addr_len: null,
        opcua: null,
        tcp_addr: null,
        tcp_concurrent: null,
        channel_capacity: 128,
        reconnect_backoff: { seconds: [1, 2, 5, 10, 30], jitter_ratio: 0 },
      } as BusConfig)
      await postConfig({ ...cfg, buses: list })
      busPick.value = port
      showNewBus.value = false
      await loadBuses()
      ElMessage.success(`已新增总线 ${port}`)
    } catch (e) {
      ElMessage.error(e instanceof Error && e.message ? e.message : '新增总线失败')
    } finally {
      addingBus.value = false
    }
  }

  return {
    buses,
    busPick,
    showNewBus,
    addingBus,
    newBus,
    selectedBus,
    loadBuses,
    addBus,
  }
}
