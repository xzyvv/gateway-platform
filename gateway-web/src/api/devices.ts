import { request } from './index'
import type { DeviceConfig } from './types'

/** 设备列表 */
export function listDevices(): Promise<DeviceConfig[]> {
  return request<DeviceConfig[]>({ url: '/api/devices', method: 'GET' })
}

/** 创建设备 */
export function createDevice(device: DeviceConfig): Promise<DeviceConfig> {
  return request<DeviceConfig>({ url: '/api/devices', method: 'POST', data: device })
}

/** 更新设备 */
export function updateDevice(id: string, device: DeviceConfig): Promise<DeviceConfig> {
  return request<DeviceConfig>({ url: `/api/devices/${id}`, method: 'PUT', data: device })
}

/** 删除设备 */
export function deleteDevice(id: string): Promise<{ status: string }> {
  return request({ url: `/api/devices/${id}`, method: 'DELETE' })
}

/** GPIO 状态（4 路 DI + 4 路 DO） */
export interface IoStatus {
  di: boolean[]
  di_quality: boolean[]
  do_state: boolean[]
  /** 当前平台是否具备真实 GPIO 能力（仅 Linux + gpiod）；false 时其余字段无物理意义 */
  platform_supported: boolean
}
export function getIoStatus(): Promise<IoStatus> {
  return request({ url: '/api/io/status', method: 'GET' })
}

/** GPIO 控制 — 拉指定数字输出引脚 on/off */
export function controlIo(index: number, action: 'on' | 'off'): Promise<{ status: string; index: number; on: boolean }> {
  return request({ url: `/api/io/${index}`, method: 'POST', data: { action } })
}
