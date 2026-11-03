import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  listTasks,
  createTask,
  updateTask,
  deleteTask,
  runTaskNow,
  type ScheduleTask,
} from './scheduledTasks'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function taskRow(patch: Partial<ScheduleTask> = {}): ScheduleTask {
  return {
    id: 't1',
    name: 'T',
    type: 'read_point',
    target: 'p1',
    cron: '* * * * *',
    enabled: true,
    runCount: 0,
    createdAt: 0,
    ...patch,
  }
}

describe('scheduledTasks（计划任务真实后端契约）', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  it('listTasks 走 GET /api/access/scheduled-tasks', async () => {
    requestMock.mockResolvedValue([])
    const rows = await listTasks()
    expect(rows).toEqual([])
    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/access/scheduled-tasks')
  })

  it('createTask 走 POST 并透传 name/type/target/cron', async () => {
    requestMock.mockResolvedValue(taskRow())
    await createTask({ name: 'T', type: 'read_point', target: 'p1', cron: '* * * * *' })
    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/access/scheduled-tasks')
    expect(cfg.data).toEqual({ name: 'T', type: 'read_point', target: 'p1', cron: '* * * * *' })
  })

  it('updateTask 走 PUT 全量更新（name/type/cron 必填），不携带 id/统计字段', async () => {
    requestMock.mockResolvedValue(taskRow({ name: 'T2' }))
    await updateTask(taskRow({ name: 'T2', runCount: 7, lastRunAt: 123 }))
    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe('/api/access/scheduled-tasks/t1')
    // 全量更新体：只含可编辑字段，不含 id/runCount/lastRunAt 等后端自维护字段
    expect(cfg.data).toEqual({
      name: 'T2',
      type: 'read_point',
      target: 'p1',
      cron: '* * * * *',
      enabled: true,
    })
  })

  it('deleteTask 走 DELETE /:id', async () => {
    requestMock.mockResolvedValue(true)
    await deleteTask('t1')
    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/access/scheduled-tasks/t1')
  })

  it('runTaskNow 走 POST /:id/run 并透传执行结果', async () => {
    requestMock.mockResolvedValue({ status: 'success' })
    const r = await runTaskNow('t1')
    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/access/scheduled-tasks/t1/run')
    expect(r.status).toBe('success')
  })

  it('runTaskNow 失败结果原样返回（由页面决定如何提示），不在此吞掉', async () => {
    requestMock.mockResolvedValue({ status: 'error', error: '点位不存在' })
    const r = await runTaskNow('t1')
    expect(r.status).toBe('error')
    expect(r.error).toBe('点位不存在')
  })
})
