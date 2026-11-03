import { ref, computed, onMounted, type Ref as VueRef } from 'vue'
import { ElMessage } from 'element-plus'
import { listDevices } from '@/api/devices'
import type { DeviceConfig, PointConfig } from '@/api/types'
import { listPoints } from '@/api/points'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import { type PointQuality } from '@/api/dataQuality'
import { fetchPointQualities } from '@/composables/data/usePointQuality'
import { getUplinkConfig, testUplink } from '@/api/uplink'
import { extractErrorMessage } from '@/utils/errorHandler'

/**
 * 链路追踪可选点位：配置点位（元数据全）与运行时点位（模板展开，仅 id/name/device_id）
 * 的并集。设备用模板展开点位时配置 API 返回空，若只看配置会导致追踪页无点可选。
 */
export type TracePoint = Pick<PointConfig, 'id'> & Partial<PointConfig>

interface ReadResult { status: string; value?: number }
interface PublishInfo { topic: string; qos: number; delivered: boolean }
interface SendResult { ok: boolean; error?: string; publish?: PublishInfo }

/**
 * DataFlowTrace 页面数据层封装
 *
 * 5 步链路追踪（设备/点位选择 → 采集 → 渲染 → 上送）的状态、computed、action
 * 集中到一个 composable，主组件只剩 template。
 */
export function useDataFlowTrace() {
  const { readOnce } = useReadPointNow()
  const DEFAULT_UPLINK_TEMPLATE = '{"device":"${device_id}","value":${value}}'
  const COPY_SUCCESS_MSG = '报文已复制到剪贴板'
  const COPY_FAIL_MSG = '复制失败，请手动选择复制'
  const STALE_CONTEXT_MSG = '设备或点位已变更，请重新采集后再上送'

  const devices = ref<DeviceConfig[]>([])
  const points = ref<TracePoint[]>([])
  const selectedDevice = ref('')
  const selectedPoint = ref('')
  /** 运行时点位（含模板展开）：/api/devices/point-quality 快照，进页拉一次 */
  const runtimePoints = ref<PointQuality[]>([])

  const device = computed(() => devices.value.find((d: DeviceConfig) => d.id === selectedDevice.value))
  const point = computed(() => points.value.find((p: TracePoint) => p.id === selectedPoint.value))

  const pointInfo = computed(() => {
    const p = point.value
    if (!p) return null
    return {
      ...p,
      data_type: typeof p.data_type === 'string' ? p.data_type : '',
      scale: typeof p.scale === 'number' ? p.scale : 1,
    }
  })

  const reading = ref(false)
  const readResult = ref<ReadResult | null>(null)
  const readLatency = ref(0)
  const readError = ref('')
  const isGood = computed(() => readResult.value?.status === 'ok' || readResult.value?.status === 'success')

  const rendering = ref(false)
  const renderedPayload = ref('')
  const renderError = ref('')
  const uplinkTemplate = ref('')
  const copying = ref(false)

  const sending = ref(false)
  const sendResult = ref<SendResult | null>(null)

  const loadingDevices = ref(false)
  const loadingPoints = ref(false)
  const loadDevicesError = ref('')
  const loadPointsError = ref('')

  const stepDone = computed(() => ({
    s1: !!device.value,
    s2: !!point.value,
    s3: !!readResult.value,
    s4: !!renderedPayload.value,
    s5: !!sendResult.value,
  }))

  type StepState = 'done' | 'active' | 'pending'
  const stepStatus = computed<StepState[]>(() => {
    const d = stepDone.value
    const state = (current: boolean, prev: boolean): StepState =>
      current ? 'done' : prev ? 'active' : 'pending'
    return [
      state(d.s1, true),
      state(d.s2, d.s1),
      state(d.s3, d.s2),
      state(d.s4, d.s3),
      state(d.s5, d.s4),
    ]
  })

  const latencyLevel = computed(() => {
    const ms = readLatency.value
    if (ms < 100) return 'fast'
    if (ms < 500) return 'normal'
    return 'slow'
  })

  function resetTrace() {
    readResult.value = null
    readLatency.value = 0
    readError.value = ''
    renderedPayload.value = ''
    renderError.value = ''
    uplinkTemplate.value = ''
    sendResult.value = null
  }

  function errorMessage(e: unknown): string {
    return extractErrorMessage(e)
  }

  function reportError(ref: VueRef<string>, prefix: string, e: unknown): void {
    const msg = errorMessage(e)
    ref.value = msg
    ElMessage.error(`${prefix}：${msg}`)
  }

  async function loadDevices() {
    loadingDevices.value = true
    loadDevicesError.value = ''
    try {
      // 运行时点位失败不阻塞设备列表：追踪页核心是设备+点位，质量快照拿不到时
      // 配置点位仍可用，只是模板展开点位会缺席
      const [devs, rtPts] = await Promise.all([
        listDevices(),
        fetchPointQualities().catch(() => [] as PointQuality[]),
      ])
      devices.value = devs
      runtimePoints.value = rtPts
    } catch (e) {
      reportError(loadDevicesError, '加载设备失败', e)
    } finally {
      loadingDevices.value = false
    }
  }

  async function onDeviceChange() {
    selectedPoint.value = ''
    loadPointsError.value = ''
    resetTrace()
    const devId = selectedDevice.value
    // 运行时点位（含模板展开）先进集合：配置接口失败时追踪仍可用
    const merged = new Map<string, TracePoint>()
    for (const rp of runtimePoints.value) {
      if (rp.device_id === devId) {
        merged.set(rp.point_id, { id: rp.point_id, name: rp.point_name, device_id: rp.device_id })
      }
    }
    points.value = []
    if (!devId) {
      loadingPoints.value = false
      return
    }
    loadingPoints.value = true
    try {
      const pts = await listPoints(devId)
      if (selectedDevice.value !== devId) return
      // 配置点位元数据更全（寄存器/数据类型/缩放），覆盖同 id 的运行时条目
      for (const p of pts) merged.set(p.id, p)
      points.value = [...merged.values()]
    } catch (e) {
      if (selectedDevice.value !== devId) return
      if (merged.size) {
        // 配置点位拉取失败但有运行时点位：降级可用，不打断追踪
        points.value = [...merged.values()]
      } else {
        reportError(loadPointsError, '加载点位失败', e)
      }
    } finally {
      loadingPoints.value = false
    }
  }

  async function doRead() {
    if (!selectedPoint.value || reading.value) return
    reading.value = true
    readResult.value = null
    readError.value = ''
    readLatency.value = 0
    renderedPayload.value = ''
    renderError.value = ''
    uplinkTemplate.value = ''
    sendResult.value = null
    try {
      const outcome = await readOnce(selectedPoint.value)
      readLatency.value = Math.round(outcome.latencyMs)
      // 保留完整原始响应供第 3 步渲染取 value/quality；传输失败（无原始响应）
      // 时 readResult 保持 null，避免用合成对象让「第 3 步」误判为已完成。
      readResult.value = (outcome.raw ?? null) as ReadResult | null
      if (!outcome.ok) readError.value = outcome.error
    } finally {
      reading.value = false
    }
  }

  function buildVars() {
    return {
      device_id: device.value?.id || '',
      point_id: selectedPoint.value,
      value: readResult.value?.value ?? 0,
      quality_code: isGood.value ? 0 : 1,
    }
  }

  async function doRender() {
    if (!readResult.value || rendering.value) return
    rendering.value = true
    renderedPayload.value = ''
    renderError.value = ''
    uplinkTemplate.value = ''
    sendResult.value = null
    try {
      const cfg = await getUplinkConfig()
      const tpl = cfg.template || DEFAULT_UPLINK_TEMPLATE
      const resp = await testUplink({ template: tpl, vars: buildVars(), send: false })
      if (resp.ok && resp.rendered) {
        uplinkTemplate.value = tpl
        renderedPayload.value = resp.rendered
      } else {
        renderError.value = resp.error || '渲染失败：未知错误'
      }
    } catch (e) {
      renderError.value = `渲染异常：${errorMessage(e)}`
    } finally {
      rendering.value = false
    }
  }

  async function doSend() {
    if (!readResult.value || !renderedPayload.value || sending.value) return
    if (!device.value || !selectedPoint.value) {
      ElMessage.error(STALE_CONTEXT_MSG)
      return
    }
    sending.value = true
    sendResult.value = null
    try {
      const tpl = uplinkTemplate.value
        || ((await getUplinkConfig()).template || DEFAULT_UPLINK_TEMPLATE)
      const resp = await testUplink({ template: tpl, vars: buildVars(), send: true })
      sendResult.value = { ok: resp.ok, error: resp.error, publish: resp.publish }
    } catch (e) {
      sendResult.value = { ok: false, error: errorMessage(e) }
    } finally {
      sending.value = false
    }
  }

  async function copyPayload() {
    if (!renderedPayload.value || copying.value) return
    copying.value = true
    const text = renderedPayload.value
    try {
      if (navigator.clipboard) {
        await navigator.clipboard.writeText(text)
        ElMessage.success(COPY_SUCCESS_MSG)
        return
      }
      const ta = document.createElement('textarea')
      ta.value = text
      ta.style.position = 'fixed'
      ta.style.opacity = '0'
      document.body.appendChild(ta)
      ta.select()
      const ok = document.execCommand('copy')
      document.body.removeChild(ta)
      if (ok) ElMessage.success(COPY_SUCCESS_MSG)
      else ElMessage.error(COPY_FAIL_MSG)
    } catch {
      ElMessage.error(COPY_FAIL_MSG)
    } finally {
      copying.value = false
    }
  }

  onMounted(loadDevices)

  return {
    // state
    devices, points, selectedDevice, selectedPoint,
    device, point, pointInfo,
    readResult, readLatency, readError, isGood, reading,
    renderedPayload, renderError, rendering, copying, uplinkTemplate,
    sendResult, sending,
    loadingDevices, loadingPoints, loadDevicesError, loadPointsError,
    stepDone, stepStatus, latencyLevel,
    // actions
    loadDevices, onDeviceChange, resetTrace, doRead, doRender, doSend, copyPayload,
  }
}
