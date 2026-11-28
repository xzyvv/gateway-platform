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
import { createPoint, listPoints } from '@/api/acquisition/points'
import {
  modbusAddressOf,
  parseModbusAddress,
} from '@/composables/devices/usePoints/modbusAddress'
import type { PointConfig } from '@/api/types'
import { defaultPointConfig } from '@/utils/config/pointDefaults'
import type { ManualPointForm } from './types'
import { extractErrorReason } from '@/utils/error/errorHandler'

export function usePointStep() {
  /** 已创建设备的点位列表 */
  const points = ref<PointConfig[]>([])
  /** 点位列表加载中 */
  const loadingPoints = ref(false)
  /** 手动补录点位的表单值 */
  const manual = ref<ManualPointForm>({ name: '', address: '', dataType: 'i16' })
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
    // 非 Modbus 协议按数据标识 / 软元件寻址，向导这套「寄存器地址」表单并不适用，
    // 硬写 address_num / function_code 只会建出采不到数的点位。
    if (!bus.protocol.toLowerCase().startsWith('modbus')) {
      ElMessage.warning('当前总线不是 Modbus，请完成向导后在「点位管理」中添加该协议的点位')
      return
    }
    const parsed = parseModbusAddress(manual.value.address)
    if (!parsed) {
      ElMessage.warning('请填写 5 位 Modbus 地址，如 40001（区段首位 + 1 基序号）')
      return
    }
    // 规范化回 5 位串：用户输入 0 号地址时会被 clamp 成 1，回显要与实际寻址一致
    const address = modbusAddressOf(parsed.functionCode, parsed.addressNum)
    addingPoint.value = true
    try {
      await createPoint(device.id, {
        ...defaultPointConfig({
          id: `${device.id}_${address}`,
          name: manual.value.name.trim(),
          device_id: device.id,
          bus: bus.port_name,
          protocol: bus.protocol,
          address,
          slave_id: slaveId,
          address_num: parsed.addressNum,
          function_code: parsed.functionCode,
          data_type: manual.value.dataType,
        }),
      })
      ElMessage.success('点位已添加')
      manual.value = { name: '', address: '', dataType: 'i16' }
      await loadPoints(device.id)
    } catch (e) {
      ElMessage.error(extractErrorReason(e, '添加点位失败'))
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
