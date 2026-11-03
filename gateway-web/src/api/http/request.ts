/**
 * 核心请求函数
 *
 * - request：透传 axios 响应体的 data（拦截器已解开 ApiResponse 信封）
 * - requestWithMeta：额外返回响应头里的离线陈旧标记（见 ResponseMeta）
 */
import type { AxiosRequestConfig } from 'axios'
import { instance } from './instance'
import { readResponseMeta, type ResponseMeta } from './trace'

export async function request<T = unknown>(config: AxiosRequestConfig): Promise<T> {
  const resp = await instance.request<T>(config)
  return resp.data
}

/** 同 request，额外返回响应头里的离线陈旧标记（见 ResponseMeta） */
export async function requestWithMeta<T = unknown>(
  config: AxiosRequestConfig,
): Promise<{ data: T; meta: ResponseMeta }> {
  const resp = await instance.request<T>(config)
  return { data: resp.data, meta: readResponseMeta(resp.headers) }
}
