/**
 * axios 实例 + 请求拦截器
 *
 * - 请求拦截器：自动附带 Bearer Token + 记录起始时间戳 + 注册 AbortController
 * - 全局 in-flight 控制：切换网关 / 登出时 abortAllInflight 把"切网之前的请求"
 *   统一取消，避免请求打到错误的网关或被静默丢弃（注册中心实现见 ../inflight）
 */
import axios, { type AxiosInstance, type InternalAxiosRequestConfig } from 'axios'
import { isTauri } from '../env'
import { tauriAdapter } from '../gatewayConfig'
import { getToken } from '../token'
import { addInflight, removeInflight } from '../inflight'

export const instance: AxiosInstance = axios.create({
  baseURL: '/',
  timeout: 15000,
  adapter: isTauri() ? tauriAdapter : undefined,
})

export interface TimedConfig extends InternalAxiosRequestConfig {
  _startAt?: number
  _abortController?: AbortController
}

instance.interceptors.request.use(
  async (config: InternalAxiosRequestConfig): Promise<InternalAxiosRequestConfig> => {
    const token = await getToken()
    if (token) {
      config.headers.set('Authorization', `Bearer ${token}`)
    }
    const cfg = config as TimedConfig
    // 修复(W-1)：无论调用方有没有自带 signal，都必须登记一个内部 AbortController。
    // 原实现只在 `!cfg.signal` 时登记，导致切网关时 abortAllInflight() 漏掉
    // 自带 signal 的请求（包括下行写 / 配置下发），而 triggerGatewaySwitchRefresh
    // 只等很短时间就 reload，这些在飞的写可能落到上一台网关。
    // 现在统一创建内部 controller，并把调用方 signal 的取消桥接进来：
    // - 调用方 abort → 内部 abort（保留原有语义）
    // - abortAllInflight()（切网关）→ 内部 abort（所有在飞请求都可被取消）
    const ctrl = new AbortController()
    const callerSignal = cfg.signal as AbortSignal | undefined
    if (callerSignal) {
      if (callerSignal.aborted) {
        ctrl.abort(callerSignal.reason)
      } else {
        // once:true 让监听在触发后自动摘除，避免长生命周期 signal 累积监听
        callerSignal.addEventListener('abort', () => ctrl.abort(callerSignal.reason), {
          once: true,
        })
      }
    }
    cfg.signal = ctrl.signal
    cfg._abortController = ctrl
    addInflight(ctrl)
    cfg._startAt = Date.now()
    return cfg
  },
  (error) => Promise.reject(error),
)

export function unregisterInflight(cfg: TimedConfig | undefined) {
  removeInflight(cfg?._abortController)
}
