import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@/api', () => ({
  isTauri: vi.fn(() => false),
  scanGatewayLan: vi.fn(),
}))

import { isTauri, scanGatewayLan } from '@/api'
import { useGatewayDiscovery } from './useGatewayDiscovery'

describe('useGatewayDiscovery', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(isTauri).mockReturnValue(false)
  })

  it('初始状态：scanning=false, found=[], error=null, elapsedMs=0', () => {
    const { scanning, found, error, elapsedMs } = useGatewayDiscovery()
    expect(scanning.value).toBe(false)
    expect(found.value).toEqual([])
    expect(error.value).toBeNull()
    expect(elapsedMs.value).toBe(0)
  })

  it('cancel 设置 scanning=false', () => {
    const { scanning, cancel } = useGatewayDiscovery()
    cancel()
    expect(scanning.value).toBe(false)
  })

  it('Tauri 路径：scanGatewayLan 成功时填充 found 列表 + preferred 标记', async () => {
    vi.mocked(isTauri).mockReturnValue(true)
    vi.mocked(scanGatewayLan).mockResolvedValue([
      { ip: '192.168.1.1', port: 8080, name: 'gw1', latency_ms: 10 },
      { ip: '192.168.1.254', port: 8080, name: 'gw2', latency_ms: 20 },
      { ip: '192.168.1.100', port: 8080, name: 'gw3', latency_ms: 15 },
    ])
    const { scan, found, scanning, elapsedMs } = useGatewayDiscovery()
    const result = await scan()
    expect(scanning.value).toBe(false)
    expect(result).toHaveLength(3)
    expect(result[0]!.url).toBe('http://192.168.1.1:8080')
    expect(result[0]!.preferred).toBe(true) // .1 → preferred
    expect(result[1]!.preferred).toBe(true) // .254 → preferred
    expect(result[2]!.preferred).toBe(false) // .100 → not preferred
    expect(elapsedMs.value).toBeGreaterThanOrEqual(0)
    expect(found.value).toEqual(result)
  })

  it('Tauri 路径：scanGatewayLan 抛异常时设置 error 并返回空列表', async () => {
    vi.mocked(isTauri).mockReturnValue(true)
    vi.mocked(scanGatewayLan).mockRejectedValue(new Error('network error'))
    const { scan, error, scanning } = useGatewayDiscovery()
    const result = await scan()
    expect(scanning.value).toBe(false)
    expect(result).toEqual([])
    expect(error.value).toBe('network error')
  })

  it('Tauri 路径：scanGatewayLan 抛非 Error 时 error 为字符串化值', async () => {
    vi.mocked(isTauri).mockReturnValue(true)
    vi.mocked(scanGatewayLan).mockRejectedValue('timeout')
    const { scan, error } = useGatewayDiscovery()
    await scan()
    expect(error.value).toBe('timeout')
  })

  it('正在扫描时再次 scan 返回当前 found（不重复启动）', async () => {
    vi.mocked(isTauri).mockReturnValue(true)
    // never resolves → scanning stays true
    vi.mocked(scanGatewayLan).mockImplementation(() => new Promise(() => {}))
    const { scan, found } = useGatewayDiscovery()
    scan() // starts scanning (not awaited)
    // give microtask a chance to set scanning=true
    await new Promise((r) => setTimeout(r, 10))
    const result = await scan() // should return found.value immediately
    expect(result).toEqual([])
    expect(found.value).toEqual([])
  })

  it('cancel 中止进行中的扫描', async () => {
    vi.mocked(isTauri).mockReturnValue(true)
    vi.mocked(scanGatewayLan).mockImplementation(() => new Promise(() => {}))
    const { scan, cancel, scanning } = useGatewayDiscovery()
    scan()
    await new Promise((r) => setTimeout(r, 10))
    cancel()
    expect(scanning.value).toBe(false)
  })
})
