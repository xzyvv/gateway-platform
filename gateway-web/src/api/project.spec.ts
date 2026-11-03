import { describe, it, expect, vi, beforeEach } from 'vitest'

const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { exportProject, importProject } from './project'
import type { ProjectFile } from './project'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function sampleProject(): ProjectFile {
  return {
    kind: 'gateway-project',
    schema_version: 1,
    exported_at: 1700000000,
    buses: [],
    northbound: { mqtt: { broker: 'proj.broker' } },
    mappings: { points: [] },
  }
}

describe('工程文件导入导出契约', () => {
  beforeEach(() => requestMock.mockReset())

  // 锁定路径契约：后端真实路径是 GET /api/project/export，
  // 防止路径回退（如误改成 /api/config/export）导致工程文件语义静默变成了整份配置。
  it('导出走 GET /api/project/export', async () => {
    requestMock.mockResolvedValue(sampleProject())
    await exportProject()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/project/export')
  })

  // 导入走 POST /api/project/import，且工程文件作为请求体原样透传。
  it('导入走 POST /api/project/import 并透传工程文件', async () => {
    requestMock.mockResolvedValue(undefined)
    const project = sampleProject()
    await importProject(project)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/project/import')
    expect(cfg.data).toBe(project)
  })

  // 原样透传后端响应（kind + 三层 + schema_version）
  it('原样透传导出响应', async () => {
    const resp = sampleProject()
    requestMock.mockResolvedValue(resp)
    await expect(exportProject()).resolves.toEqual(resp)
  })
})
