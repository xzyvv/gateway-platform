/**
 * 错误提示口径：写操作（非 GET）失败时全局静默，只写通知中心，由调用方给出
 * 带动作语境的提示；GET（轮询/加载）无人负责提示，仍由全局弹出。
 * 401 / 4021 / 429 属于跨页面全局状态，保持全局提示。
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { AxiosError, type AxiosResponse, type InternalAxiosRequestConfig } from 'axios'
import { ElMessage, ElNotification } from 'element-plus'

vi.mock('element-plus', () => {
  const message = Object.assign(vi.fn(), {
    warning: vi.fn(),
    error: vi.fn(),
    success: vi.fn(),
    info: vi.fn(),
  })
  return { ElMessage: message, ElNotification: vi.fn() }
})

import { instance, request } from './http'
import { useNotificationsStore } from '../stores/notifications'

const realAdapter = instance.defaults.adapter

function failWith(status: number, data: unknown, code = 'ERR_BAD_RESPONSE') {
  instance.defaults.adapter = async (config) => {
    const response = {
      status,
      statusText: 'ERR',
      headers: {},
      config,
      data,
    } as AxiosResponse
    throw new AxiosError('Request failed', code, config as InternalAxiosRequestConfig, undefined, response)
  }
}

function failNetwork(code = 'ERR_NETWORK') {
  instance.defaults.adapter = async (config) => {
    throw new AxiosError('Network Error', code, config as InternalAxiosRequestConfig)
  }
}

describe('写操作失败全局静默（isMutation）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  afterEach(() => {
    instance.defaults.adapter = realAdapter
  })

  it('GET 失败仍弹全局通知，并写入通知中心', async () => {
    failWith(500, { error: 'boom' })

    await expect(request({ url: '/api/probe', method: 'GET' })).rejects.toThrow()

    const store = useNotificationsStore()
    expect(store.items).toHaveLength(1)
    expect(store.items[0]!.message).toBe('请求失败(500): boom')
    expect(ElNotification).toHaveBeenCalledTimes(1)
  })

  it('POST 失败只留痕通知中心，不弹窗（由调用方给出动作语境提示）', async () => {
    failWith(500, { error: 'save failed' })

    await expect(request({ url: '/api/save', method: 'POST' })).rejects.toThrow()

    const store = useNotificationsStore()
    expect(store.items).toHaveLength(1)
    expect(store.items[0]!.message).toBe('请求失败(500): save failed')
    expect(ElNotification).not.toHaveBeenCalled()
  })

  it('DELETE 网络错误同样静默', async () => {
    failNetwork()

    await expect(request({ url: '/api/items/1', method: 'DELETE' })).rejects.toThrow()

    const store = useNotificationsStore()
    expect(store.items[0]!.message).toContain('网络错误')
    expect(ElNotification).not.toHaveBeenCalled()
  })

  it('GET 网络错误仍弹窗', async () => {
    failNetwork()

    await expect(request({ url: '/api/poll', method: 'GET' })).rejects.toThrow()

    expect(ElNotification).toHaveBeenCalledTimes(1)
  })

  it('429 限流对写操作仍全局提示（全局状态例外）', async () => {
    failWith(429, { error: 'slow down' }, 'ERR_BAD_REQUEST')

    await expect(request({ url: '/api/save', method: 'POST' })).rejects.toThrow()

    expect(ElMessage.warning).toHaveBeenCalledWith('请求过于频繁，请稍后重试')
  })
})
