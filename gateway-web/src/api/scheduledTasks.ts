/**
 * 计划任务（网关真实后端：/api/access/scheduled-tasks）
 *
 * 任务持久化在网关 SQLite，由网关内置调度器（15s tick）真实执行：
 * 读取点位 / 读取设备 / 备份配置 / 同步 NTP，单任务 60s 超时，到点串行执行。
 * 下次执行时间（nextRunAt）由后端按 cron 表达式计算，前端不再推算。
 */
import { request } from './index'

export type TaskType = 'read_point' | 'read_device' | 'backup_config' | 'sync_ntp'

export interface ScheduleTask {
  id: string
  name: string
  type: TaskType
  target?: string
  cron: string
  enabled: boolean
  lastRunAt?: number
  nextRunAt?: number
  runCount: number
  lastStatus?: 'success' | 'error'
  lastError?: string
  createdAt: number
}

export interface RunNowResult {
  status: 'success' | 'error'
  error?: string
}

const CRON_PRESETS = [
  { label: '每分钟', value: '* * * * *' },
  { label: '每5分钟', value: '*/5 * * * *' },
  { label: '每15分钟', value: '*/15 * * * *' },
  { label: '每小时', value: '0 * * * *' },
  { label: '每天2点', value: '0 2 * * *' },
  { label: '每周一', value: '0 0 * * 1' },
]

export function getCronPresets() {
  return CRON_PRESETS
}

export function listTasks(): Promise<ScheduleTask[]> {
  return request({ url: '/api/access/scheduled-tasks', method: 'GET' })
}

export function createTask(t: {
  name: string
  type: TaskType
  target?: string
  cron: string
  enabled?: boolean
}): Promise<ScheduleTask> {
  return request({ url: '/api/access/scheduled-tasks', method: 'POST', data: t })
}

/** 后端为全量更新（PUT 语义）：name/type/cron 必填，enabled 缺省沿用原值 */
export function updateTask(t: ScheduleTask): Promise<ScheduleTask> {
  return request({
    url: `/api/access/scheduled-tasks/${t.id}`,
    method: 'PUT',
    data: { name: t.name, type: t.type, target: t.target, cron: t.cron, enabled: t.enabled },
  })
}

export function deleteTask(id: string): Promise<void> {
  return request({ url: `/api/access/scheduled-tasks/${id}`, method: 'DELETE' })
}

/** 立即在网关上执行一次任务（同步等待结果，结果回写任务记录） */
export function runTaskNow(id: string): Promise<RunNowResult> {
  return request({ url: `/api/access/scheduled-tasks/${id}/run`, method: 'POST' })
}
