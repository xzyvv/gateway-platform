import { request } from './index'

/** D5 模板库 */
export interface TemplatePoint {
  name: string
  address?: string
  /** 单位决定 CT/PT 变比是否生效（电压只吃 PT、电流只吃 CT、功率/电量两者都吃） */
  unit?: string
  data_type?: string
  /** 0 基寄存器偏移（Modbus）/ IOA（IEC104），展开后成为点位的 address_num */
  default_address?: number
  data_id?: string
  /** Modbus 功能码：决定读区与展示地址区段 */
  function_code?: number
  register_count?: number
  scale?: number
  offset?: number
  formula?: string
  deadband?: number
  /** CT/PT 变比：留空=不启用，填了须 > 0 */
  ct_ratio?: number | null
  pt_ratio?: number | null
  [key: string]: unknown
}
export interface DeviceTemplate {
  name: string
  vendor?: string
  model?: string
  /** 模板说明：随模板文件分发，纯文档字段，不参与采集 */
  description?: string
  protocol?: string
  default_interval?: number
  points: TemplatePoint[]
  [key: string]: unknown
}
export function listTemplates(): Promise<DeviceTemplate[]> {
  return request({ url: '/api/templates', method: 'GET' })
}
export function saveTemplate(t: DeviceTemplate): Promise<{status:string}> {
  return request({ url: '/api/templates', method: 'POST', data: t })
}
export function deleteTemplate(name: string): Promise<{status:string}> {
  return request({ url: `/api/templates/${encodeURIComponent(name)}`, method: 'DELETE' })
}
