import { request } from './index'

/** D5 模板库 */
export interface TemplatePoint {
  name: string
  address: string
  register_type?: string
  registerType?: string
  data_type?: string
  dataType?: string
  [key: string]: unknown
}
export interface DeviceTemplate { name: string; protocol?: string; default_interval?: number; points: TemplatePoint[]; [key:string]: unknown }
export function listTemplates(): Promise<DeviceTemplate[]> {
  return request({ url: '/api/templates', method: 'GET' })
}
export function saveTemplate(t: DeviceTemplate): Promise<{status:string}> {
  return request({ url: '/api/templates', method: 'POST', data: t })
}
export function deleteTemplate(name: string): Promise<{status:string}> {
  return request({ url: `/api/templates/${encodeURIComponent(name)}`, method: 'DELETE' })
}
