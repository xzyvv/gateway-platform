/**
 * 部署向导 · 第 3 步（配置点位）的状态与真实接口调用。
 *
 * 点位列表来自真实设备点位接口，手动补录的点位也是真实写入。
 * 这里持有点位相关的全部响应式状态，页面只把状态透传给子组件渲染。
 *
 * 典型应用场景：部署向导的第 3 步（配置点位）。
 */
import { ref } from 'vue'
import { ElMessage } from 'element-plus'
import { createPoint, listPoints } from '@/api/points'
import type { PointConfig } from '@/api/types'
import type { ManualPointForm } from './types'

export function usePointStep() {
  /** 已创建设备的点位列表 */
  const points = ref<PointConfig[]>([])
  /** 点位列表加载中 */
  const loadingPoints = ref(false)
  /** 手动补录点位的表单值 */
  const manual = ref<ManualPointForm>({ name: '', addressNum: 0, dataType: 'i16' })
  /** 添加点位请求进行中 */
  const addingPoint = ref(false)

  /** 读取指定设备的点位列表 */
  async function loadPoints(deviceId: string | undefined) {
    if (!deviceId) return
    loadingPoints.value = true
    try {
      points.value = await listPoints(deviceId)
    } catch {
      points.value = []
    } finally {
      loadingPoints.value = false
    }
  }

  /** 手动补录一个点位（真实写入） */
  async function addPoint(
    device: { id: string } | null,
    bus: { port_name: string; protocol: string } | undefined,
    slaveId: number,
  ) {
    if (!device || !bus) return
    if (!manual.value.name.trim()) {
      ElMessage.warning('请填写点位名称')
      return
    }
    const regAddr = manual.value.addressNum
    addingPoint.value = true
    try {
      await createPoint(device.id, {
        id: `${device.id}_${regAddr}`,
        name: manual.value.name.trim(),
        device_id: device.id,
        bus: bus.port_name,
        protocol: bus.protocol,
        address: String(regAddr),
        slave_id: slaveId,
        address_num: 1,
        data_id: null,
        function_code: 3,
        data_type: manual.value.dataType,
        byte_order: null,
        register_count: null,
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
      ElMessage.success('点位已添加')
      manual.value = { name: '', addressNum: 0, dataType: 'i16' }
      await loadPoints(device.id)
    } catch (e) {
      ElMessage.error(e instanceof Error && e.message ? e.message : '添加点位失败')
    } finally {
      addingPoint.value = false
    }
  }

  return {
    points,
    loadingPoints,
    manual,
    addingPoint,
    loadPoints,
    addPoint,
  }
}
