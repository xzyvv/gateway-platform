/**
 * 设备连通性检测与在线状态
 *
 * 含批量连通性检测（并发 8，逐设备读点一次）、
 * 表单内单设备连通性探测（走总线扫描端点），
 * 以及设备通信质量徽章（颜色/文案委托全站唯一映射 utils/statusType）。
 */
import { computed } from 'vue'
import { ElMessage } from 'element-plus'
import { scanBus, respondedSlaves } from '@/api/protocolTools/slaveScan'
import { listPoints } from '@/api/points'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import { getDeviceStats, commsQuality, recordRead as recordDeviceRead, type DeviceCommsStat } from '@/utils/deviceComms'
import { recordBusOp } from '@/utils/busStats'
import { extractErrorMessage } from '@/utils/errorHandler'
import { statusTagType, type TagType } from '@/utils/statusType'
import type { DeviceConfig } from '@/api/types'
import type { DeviceConnectivityDeps, DeviceState } from './types'

/**
 * 设备连通性检测与在线状态
 *
 * @param state 共享状态（devices / form / formRef / testingConn / checking / audit / 卸载守卫）
 * @param deps  跨模块依赖（协议判定），由 index.ts 注入避免循环引用
 */
export function createDeviceConnectivity(state: DeviceState, deps: DeviceConnectivityDeps) {
  const { readOnce } = useReadPointNow()
  // 设备通信质量统计（来自 deviceComms 工具聚合）
  const commsStats = computed<Record<string, DeviceCommsStat>>(() => {
    const map: Record<string, DeviceCommsStat> = {}
    for (const s of getDeviceStats()) map[s.deviceId] = s
    return map
  })
  /**
   * 通信质量 → 徽章颜色：good/warn/bad/unknown 已登记在全站唯一映射表 utils/statusType，
   * 这里直接委托，避免同一状态在别处取到不同颜色。
   */
  function commsQualityType(id: string): TagType {
    const s = commsStats.value[id]
    if (!s) return 'info'
    return statusTagType(commsQuality(s))
  }
  function commsQualityText(id: string): string {
    const s = commsStats.value[id]
    if (!s) return '—'
    const q = commsQuality(s)
    return q === 'good' ? '良好' : q === 'warn' ? '一般' : q === 'bad' ? '异常' : '未知'
  }

  async function batchCheck() {
    if (state.devices.value.length === 0) {
      ElMessage.warning('暂无设备可检测')
      return
    }
    state.checking.value = true
    const CONCURRENCY = 8
    const queue = [...state.devices.value]
    let ok = 0
    let fail = 0
    const results: { name: string; reachable: boolean; latency?: number }[] = []

    async function checkOne(dev: DeviceConfig) {
      if (state.isUnmounted()) return
      try {
        const pts = await listPoints(dev.id)
        if (pts.length === 0) {
          results.push({ name: dev.name, reachable: false })
          fail++
          return
        }
        const outcome = await readOnce(pts[0]!.id)
        if (outcome.ok) {
          results.push({ name: dev.name, reachable: true, latency: outcome.latencyMs })
          ok++
          recordDeviceRead(dev.id, true, outcome.latencyMs)
          recordBusOp(dev.bus, true, outcome.latencyMs)
        } else {
          results.push({ name: dev.name, reachable: false })
          fail++
          recordDeviceRead(dev.id, false, outcome.latencyMs)
          recordBusOp(dev.bus, false, outcome.latencyMs, 'read_error')
        }
      } catch {
        results.push({ name: dev.name, reachable: false })
        fail++
        recordDeviceRead(dev.id, false, 0)
        recordBusOp(dev.bus, false, 0, 'connect_error')
      }
    }

    async function worker() {
      while (!state.isUnmounted()) {
        const dev = queue.shift()
        if (!dev) break
        await checkOne(dev)
      }
    }
    await Promise.all(
      Array.from({ length: Math.min(CONCURRENCY, state.devices.value.length) }, () => worker()),
    )

    if (!state.isUnmounted()) {
      state.checking.value = false
      const offline = results.filter((r) => !r.reachable).map((r) => r.name).join('、')
      ElMessage.success(`检测完成：在线 ${ok}，离线 ${fail}${offline ? `（${offline}）` : ''}`)
    }
  }

  async function testDeviceConn() {
    if (!state.formRef.value) return
    try {
      await state.formRef.value.validate()
    } catch {
      return
    }
    // 从站号应取 form.slave_id；Modbus 未单独填写时回退到地址字段（地址即从站号）。
    // 表计类协议（DLT645/CJT188）的地址是表号，不能当从站号用，必须显式填写。
    const slave = state.form.slave_id ?? (deps.isModbus() ? Number(state.form.address) : NaN)
    if (!Number.isInteger(slave) || slave < 1 || slave > 247) {
      ElMessage.warning('请先填写从站号（Modbus 从站范围 1-247）')
      return
    }
    state.testingConn.value = true
    try {
      const t0 = performance.now()
      // P0-2：单从站连通性探测复用同一扫描端点（只探测该地址）
      const results = await scanBus(state.form.bus, slave, slave)
      const latency = Math.round(performance.now() - t0)
      const found = respondedSlaves(results).length > 0
      if (found) {
        ElMessage.success(`设备可达！总线 ${state.form.bus} 地址 ${slave} 响应正常，耗时 ${latency}ms`)
      } else {
        ElMessage.error(`设备无响应。总线 ${state.form.bus} 上未探测到地址 ${slave} 的设备，请检查接线、波特率或从站号`)
      }
    } catch (e) {
      ElMessage.error(`探测失败：${extractErrorMessage(e)}`)
    } finally {
      state.testingConn.value = false
    }
  }

  return { commsStats, commsQualityType, commsQualityText, batchCheck, testDeviceConn }
}
