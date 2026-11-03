import { ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { getConfig, postConfig } from '@/api/config'
import { protocolType, protocolDefaultTimeoutMs } from './useProtocol'
import { createDevice } from '@/api/devices'
import { scanBus, respondedSlaves } from '@/api/protocolTools/slaveScan'
import type { SlaveScanResult } from '@/api/protocolTools/types'
import type { BusConfig, DeviceConfig } from '@/api/types'

interface FoundDevice {
  uid: string
  slave_id: number
  protocol: string
  name: string
  id: string
  bus: string
}

const IMPORT_CONCURRENCY = 6

/**
 * DiscoverWizard 页面数据层封装
 *
 * 把 DiscoverWizard.vue 中的 4 步向导（选总线 → 设置扫描范围 → 扫描结果 → 导入确认）、
 * 扫描范围计算、协议标签颜色、批量并发导入、跳页控制、$router 跳转全部
 * 集中到一个 composable，主组件只剩 template。
 */
export function useDiscoverWizard() {
  const router = useRouter()
  const step = ref(0)
  const scanning = ref(false)
  const importing = ref(false)
  const importDone = ref(false)
  const importedCount = ref(0)

  const buses = ref<BusConfig[]>([])
  const busNames = computed(() => buses.value.map((b) => b.port_name))
  // 行形状附加 name=port_name，供向导表格与选择逻辑消费
  const busTable = computed(() => buses.value.map((b) => ({ name: b.port_name, ...b })))
  const selectedBus = ref<{ name: string } & BusConfig | null>(null)

  const scanForm = ref({ start: 1, end: 247 })
  const scanCount = computed(() => Math.max(0, scanForm.value.end - scanForm.value.start + 1))
  const scanResult = ref<{ status: string; bus: string; scanned: number; found: SlaveScanResult[] }>({
    status: '', bus: '', scanned: 0, found: [],
  })

  const foundDevices = ref<FoundDevice[]>([])
  const selectedDevices = ref<FoundDevice[]>([])

  function onSelectionChange(rows: FoundDevice[]) {
    selectedDevices.value = rows
  }

  function selectBus(row: { name: string } & BusConfig) {
    selectedBus.value = { ...row }
    scanForm.value.start = 1
    scanForm.value.end = 247
    if (row.protocol && row.protocol.toLowerCase() === 'dlt645') {
      scanForm.value.end = 247
    }
    step.value = 1
  }

  async function loadBuses() {
    try {
      const cfg = await getConfig()
      buses.value = Array.isArray(cfg.buses) ? cfg.buses : []
    } catch {
      ElMessage.error('加载总线配置失败')
    }
  }

  // ===== P2c: 向导内就地新增总线 =====
  // 原实现无总线时把用户踢到「系统设置 → 运行参数」填 6 个字段再自己走回来，
  // 上下文切换成本高、容易走丢。这里提供最小可用的三字段（端口/波特率/协议）快速建档，
  // 其余串口参数按工业默认值填充，需要精调再去运行参数页修改。
  const quickAdding = ref(false)
  const quickBus = ref({ port_name: '', baud_rate: 9600, protocol: 'modbus' })

  async function quickAddBus(): Promise<boolean> {
    const port = quickBus.value.port_name.trim()
    if (!port) {
      ElMessage.warning('请先选择或输入串口端口')
      return false
    }
    if (buses.value.some((b) => b.port_name === port)) {
      ElMessage.warning(`端口 ${port} 已存在，请直接在上方选择该总线`)
      return false
    }
    quickAdding.value = true
    try {
      const cfg = await getConfig()
      const list: BusConfig[] = Array.isArray(cfg.buses) ? [...cfg.buses] : []
      list.push({
        port_name: port,
        baud_rate: quickBus.value.baud_rate,
        parity: 'none',
        data_bits: 8,
        stop_bits: 1,
        timeout_ms: protocolDefaultTimeoutMs(quickBus.value.protocol),
        protocol: quickBus.value.protocol,
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
      ElMessage.success(`已新增总线 ${port}（其余参数为默认值，可在运行参数页精调）`)
      await loadBuses()
      return true
    } catch (e) {
      ElMessage.error(`${e instanceof Error && e.message ? e.message : '新增总线失败'}，请到「系统设置 → 运行参数」手动新增`)
      return false
    } finally {
      quickAdding.value = false
    }
  }

  async function doScan() {
    if (!selectedBus.value) { ElMessage.warning('请先选择总线'); return }
    scanning.value = true
    try {
      // P0-2：扫描统一走 POST /api/protocol-tools/slave-scan（与「从站扫描」页同一实现）
      const results = await scanBus(selectedBus.value.name, scanForm.value.start, scanForm.value.end)
      const found = respondedSlaves(results)
      const busName = selectedBus.value.name
      const busProto = selectedBus.value.protocol || 'modbus'
      scanResult.value = { status: 'ok', bus: busName, scanned: results.length, found }
      foundDevices.value = found.map((item, idx) => ({
        uid: `${busName}_${item.slaveId}_${idx}`,
        slave_id: item.slaveId,
        protocol: busProto,
        name: `${busName}_slave_${item.slaveId}`,
        id: '',
        bus: busName,
      }))
      step.value = 2
      ElMessage.success(`扫描完成：发现 ${foundDevices.value.length} 台设备`)
    } catch {
      ElMessage.error('扫描失败，请检查总线连接')
    } finally {
      scanning.value = false
    }
  }

  async function doImport() {
    if (selectedDevices.value.length === 0) { ElMessage.warning('请至少选择一台设备'); return }
    importing.value = true
    const queue = [...selectedDevices.value]
    let success = 0
    let failed = 0
    async function worker() {
      while (true) {
        const dev = queue.shift()
        if (!dev) break
        const device: DeviceConfig = {
          id: dev.id || `${dev.bus}_slave_${dev.slave_id}`,
          name: dev.name || `Slave_${dev.slave_id}`,
          template: '',
          bus: dev.bus,
          protocol: dev.protocol as DeviceConfig['protocol'],
          address: String(dev.slave_id),
          slave_id: dev.slave_id,
        } as DeviceConfig
        try {
          await createDevice(device)
          success++
        } catch {
          failed++
        }
      }
    }
    await Promise.all(
      Array.from({ length: Math.min(IMPORT_CONCURRENCY, selectedDevices.value.length) }, () => worker()),
    )
    importedCount.value = success
    importDone.value = true
    importing.value = false
    if (failed > 0) {
      ElMessage.warning(`导入完成：成功 ${success} 台，失败 ${failed} 台`)
    } else {
      ElMessage.success(`导入完成：成功 ${success} 台`)
    }
  }

  function reset() {
    step.value = 0
    selectedBus.value = null
    scanResult.value = { status: '', bus: '', scanned: 0, found: [] }
    foundDevices.value = []
    selectedDevices.value = []
    importDone.value = false
    importedCount.value = 0
    loadBuses()
  }

  function goDevices() {
    router.push('/devices')
  }

  onMounted(() => {
    loadBuses()
  })

  return {
    // state
    step, scanning, importing, importDone, importedCount,
    busTable, selectedBus, scanForm, scanResult,
    foundDevices, selectedDevices, busNames,
    quickAdding, quickBus,
    // computed
    scanCount,
    // helpers
    protocolTagType: protocolType,
    // actions
    onSelectionChange, selectBus, doScan, doImport, reset, goDevices, loadBuses, quickAddBus,
  }
}
