/**
 * 寄存器扫描页状态机（从 RegisterScan.vue 拆出）
 *
 * 负责：扫描参数（总线/从站/寄存器类型/地址区间）、启动与进度轮询、
 * 地址分布图降采样与单元格文案、导出 CSV、点位建议、扫描历史增删。
 * 页面组件只保留参数表单与卡片布局，地址分布图交给 scan/parts/RegisterScanGrid.vue。
 */
import { computed, h, onMounted, ref, type Ref } from 'vue'
import { ElButton, ElMessage, ElMessageBox, ElTag, type FormInstance } from 'element-plus'
import { setFormFieldErrors, clearFormFieldErrors } from '@/utils/formFieldError'
import {
  startScan, getScanResult, listScanResults, deleteScanResult, generatePointSuggestions,
  type ScanResult, type RegType,
} from '@/api/protocolTools'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import { useProtocolCaps } from '@/composables/acquisition/useProtocolCaps'
import { downloadTableCsv } from '@/utils/exportCsv'
import { withDiagHint } from '@/utils/diagHints'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { usePolling } from '@/composables/core/usePolling'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn, timeColumn } from '@/utils/commonColumns'

/** 点位建议行 */
export type SuggestionRow = { id: string; name: string; address: string; address_num: number }

/** 地址分布图的一个显示格（降采样后可能是多个地址的聚合）。 */
export interface GridCell {
  address: number
  endAddress: number
  responded: boolean
  value?: number | null
  error?: string | null
  hit: number
  total: number
}

/** 地址分布图的降采样结果：stride=1 表示逐地址原样显示 */
export interface GridInfo {
  stride: number
  cells: GridCell[]
}

/**
 * 降采样阈值：地址区间最大 65536（0-65535），逐格渲染会产生最多 6 万个 DOM，
 * 大区间扫描会卡死浏览器。超过阈值时按块聚合成有限个格子显示。
 */
export const MAX_GRID_CELLS = 1000

/** 扫描进度轮询间隔（ms）：扫描是后台任务，1s 一次即可，避免 300ms 高频轮询空耗 CPU。 */
const POLL_INTERVAL_MS = 1000

export function cellLabel(cell: GridCell, stride: number): string {
  return stride > 1 ? `${cell.address}+` : `${cell.address}`
}

export function cellSub(cell: GridCell, stride: number): string {
  return stride > 1 ? `${cell.hit}/${cell.total}` : `${cell.value ?? ''}`
}

export function cellTitle(cell: GridCell, stride: number): string {
  if (stride > 1) {
    return `地址 ${cell.address}-${cell.endAddress}: ${cell.hit}/${cell.total} 有响应`
  }
  // 审查#6：无响应格若有后端错误详情，拼诊断建议，现场不用再去翻 CSV
  if (!cell.responded && cell.error) return `地址 ${cell.address}: ${withDiagHint(cell.error)}`
  return `地址 ${cell.address}${cell.responded ? `: 值=${cell.value}` : ': 无响应'}`
}

/**
 * 把扫描结果的原始地址格降采样为有限个显示格。
 * - 原始格数 ≤ MAX_GRID_CELLS：原样显示（stride=1）；
 * - 超过阈值：按 stride 个地址聚合成一格，响应与否取块内「任一有响应」。
 */
function computeGrid(raw: ScanResult['cells']): GridInfo {
  if (raw.length <= MAX_GRID_CELLS) {
    return {
      stride: 1,
      cells: raw.map((c) => ({
        address: c.address, endAddress: c.address, responded: c.responded,
        value: c.value, error: c.error, hit: c.responded ? 1 : 0, total: 1,
      })),
    }
  }
  const stride = Math.ceil(raw.length / MAX_GRID_CELLS)
  const cells: GridCell[] = []
  for (let i = 0; i < raw.length; i += stride) {
    const block = raw.slice(i, i + stride)
    const first = block[0]
    const last = block[block.length - 1]
    if (!first || !last) continue
    const hit = block.filter((c) => c.responded).length
    cells.push({
      address: first.address,
      endAddress: last.address,
      responded: hit > 0,
      hit,
      total: block.length,
    })
  }
  return { stride, cells }
}

/**
 * formRef 由页面传入：模板 ref 只能绑定在页面里，跨字段校验高亮的具体调用
 * 仍集中在这里（startScanFn），页面不重复实现校验逻辑。
 */
export function useRegisterScan(formRef: Ref<FormInstance | undefined>) {
  // 共享调试目标（审查#5）：总线选择跨调试页保持一致，选项加载后失效值自动回退第一项
  const { buses, busOptions, loading: targetsLoading, selectedBus: bus } = useDebugTargets()
  // B3：能力位门控（注册表驱动）——只有声明 supports_register_scan 的协议才允许扫描，
  // 元数据缺失时按不支持处理，避免给出「点了必然失败」的按钮。
  const { loaded: capsLoaded, failed: capsFailed, capsOf } = useProtocolCaps()
  const selectedBusProtocol = computed(
    () => buses.value.find((b) => b.port_name === bus.value)?.protocol ?? '',
  )
  const scanSupported = computed(() => capsOf(selectedBusProtocol.value)?.supports_register_scan ?? false)
  const slaveId = ref(1)
  const regType = ref<RegType>('holding')
  const startAddr = ref(0)
  const endAddr = ref(99)
  const scanning = ref(false)
  /** 扫描失败原因（带处置建议）：常驻在页面，便于现场对照处理，而不是一闪而过的 toast */
  const scanError = ref('')
  const currentScan = ref<ScanResult | null>(null)
  const history = ref<ScanResult[]>([])
  const showSuggestions = ref(false)
  /** 生成点位建议进行中：网关推理往返期间禁用按钮，避免重复点击发起多次请求 */
  const suggesting = ref(false)
  const suggestions = ref<SuggestionRow[]>([])
  const gridInfo = computed<GridInfo>(() => computeGrid(currentScan.value?.cells ?? []))

  const { message } = useErrorDisplay()
  /** 轮询连续失败次数：网关掉线时避免永远停在"扫描中" */
  let pollFailures = 0
  /** 当前轮询的扫描任务 id（usePolling 的 fn 通过闭包读取） */
  let activeScanId = ''

  async function pollScanResult() {
    try {
      const r = await getScanResult(activeScanId)
      pollFailures = 0
      if (r) {
        currentScan.value = r
        if (r.status === 'done') {
          scanning.value = false
          stopPoll()
          loadHistory()
        }
      }
    } catch (e) {
      // 网关/总线掉线时轮询会持续失败：连续 3 次即停止并给出处置建议
      if (++pollFailures >= 3) {
        stopPoll()
        scanning.value = false
        scanError.value = message(e, '获取扫描进度失败')
      }
    }
  }

  // Bug 178: 定时器单例与卸载清理由 usePolling 保证（重复 start 幂等）。
  // 失败上限（3 次即停）由业务自管，因此关闭内置退避；标签页隐藏暂停仍然生效。
  const { start: startPoll, stop: stopPoll } = usePolling(pollScanResult, POLL_INTERVAL_MS, { autoStart: false, backoff: false })

  async function startScanFn() {
    if (!bus.value) { ElMessage.warning('请先选择总线'); return }
    if (!scanSupported.value) { ElMessage.warning('当前总线协议不支持寄存器扫描'); return }
    // ROI: 跨字段校验同时高亮涉及的字段（原实现只弹 toast）
    if (endAddr.value < startAddr.value) {
      setFormFieldErrors(formRef.value!, {
        startAddr: '不能大于结束地址',
        endAddr: '不能小于起始地址',
      })
      ElMessage.warning('结束地址必须大于等于起始地址')
      return
    }
    clearFormFieldErrors(formRef.value!, ['startAddr', 'endAddr'])
    scanning.value = true
    scanError.value = ''
    // Bug 178: 重复点击「开始扫描」前先停掉上一轮轮询（usePolling.stop 幂等）
    stopPoll()
    pollFailures = 0
    try {
      activeScanId = await startScan(bus.value, slaveId.value, regType.value, startAddr.value, endAddr.value)
      // 查询扫描结果
      startPoll()
    } catch (e) {
      scanning.value = false
      scanError.value = message(e, '扫描请求失败')
    }
  }

  function copySuggestion(row: { address: string; address_num: number }) {
    const text = `地址: ${row.address}, 寄存器数量: ${row.address_num}`
    navigator.clipboard?.writeText(text)
    ElMessage.success('已复制到剪贴板')
  }

  async function loadHistory() {
    history.value = await listScanResults()
  }

  /**
   * 删除一条扫描记录。后端语义是「先中止（若仍在扫）再移除」，
   * 因此这里对进行中的记录也允许删除，并在确认框里说明。
   */
  async function removeScan(row: ScanResult) {
    try {
      await ElMessageBox.confirm(
        `删除扫描记录（${row.bus} / 从站 ${row.slaveId} / ${row.startAddr}-${row.endAddr}）？` +
        (row.status !== 'done' ? '该扫描仍在进行中，删除会先中止它。' : ''),
        '删除扫描结果',
        { type: 'warning', confirmButtonText: '删除', cancelButtonText: '取消' },
      )
    } catch {
      return // 用户取消
    }
    try {
      await deleteScanResult(row.id)
      // 删除的是当前展示的记录时一并清空，避免详情停在已不存在的扫描上
      if (currentScan.value?.id === row.id) {
        currentScan.value = null
        stopPoll()
        scanning.value = false
      }
      await loadHistory()
      ElMessage.success('已删除扫描记录')
    } catch (e) {
      ElMessage.error(message(e, '删除扫描记录失败'))
    }
  }

  /**
   * 导出当前扫描结果为 CSV：现场交付证据（发厂商/附工单），
   * 统一走 downloadTableCsv（公式注入防护 + BOM）。
   */
  function exportScan() {
    const s = currentScan.value
    if (!s) return
    // 文件名安全化：bus 是 port_name（如 /dev/ttyUSB0），含路径分隔符会导致下载失败
    const safeBus = s.bus.replace(/[^\w.-]+/g, '_')
    downloadTableCsv(
      ['总线', '从站', '寄存器类型', '地址', '是否响应', '值', '功能码', '错误'],
      s.cells.map((c) => [s.bus, s.slaveId, s.regType, c.address, c.responded ? '是' : '否', c.value ?? '', c.funcCode ?? '', c.error ?? '']),
      `register-scan-${safeBus}-${s.slaveId}-${new Date(s.createdAt).toISOString().slice(0, 19).replace(/[:T]/g, '-')}.csv`,
    )
    ElMessage.success(`已导出 ${s.cells.length} 条扫描结果`)
  }

  async function viewSuggestions() {
    if (!currentScan.value) return
    suggesting.value = true
    try {
      suggestions.value = await generatePointSuggestions(currentScan.value)
      showSuggestions.value = true
    } finally {
      suggesting.value = false
    }
  }

  // 点位建议表
  const suggestionColumns: ColumnDef<SuggestionRow>[] = [
    { key: 'name', title: '点位名称建议' },
    { key: 'address', title: '起始地址', width: 100 },
    { key: 'address_num', title: '寄存器数量', width: 100 },
    actionColumn<SuggestionRow>({
      width: 100,
      cell: (row) => h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => copySuggestion(row) }, () => '复制'),
    }),
  ]

  // 扫描历史表
  const historyColumns: ColumnDef<ScanResult>[] = [
    timeColumn<ScanResult>('createdAt'),
    { key: 'bus', title: '总线', width: 180 },
    { key: 'slaveId', title: '从站', width: 80 },
    { key: 'startAddr', title: '地址范围', width: 140, cell: (row) => `${row.startAddr} - ${row.endAddr}` },
    { key: 'responded', title: '响应/总数', width: 120, cell: (row) => `${row.responded} / ${row.total}` },
    {
      key: 'status',
      title: '状态',
      width: 90,
      cell: (row) => h(ElTag, { size: 'small', type: row.status === 'done' ? 'success' : 'warning' }, () => (row.status === 'done' ? '完成' : '进行中')),
    },
    actionColumn<ScanResult>({
      width: 90,
      cell: (row) => h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => removeScan(row) }, () => '删除'),
    }),
  ]

  onMounted(loadHistory)
  // Bug 178: 轮询卸载清理由 usePolling 内部 onUnmounted 负责，无需手动处理。

  return {
    busOptions,
    targetsLoading,
    bus,
    capsLoaded,
    capsFailed,
    selectedBusProtocol,
    scanSupported,
    slaveId,
    regType,
    startAddr,
    endAddr,
    scanning,
    scanError,
    currentScan,
    history,
    showSuggestions,
    suggesting,
    suggestions,
    suggestionColumns,
    historyColumns,
    gridInfo,
    startScanFn,
    exportScan,
    viewSuggestions,
  }
}
