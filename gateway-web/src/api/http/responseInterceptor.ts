/**
 * 响应拦截器
 *
 * - 解开 ApiResponse{code,message,data} 包装、统一错误处理、
 *   401 单飞登出、429 限速提示、写入请求日志
 *
 * 依赖 ./instance 提供的 axios 实例与 TimedConfig；本模块被 ./index 以副作用方式
 * 引入，保证任何经由 '@/api/http' 拿到的实例都已挂好响应拦截器。
 */
import axios from 'axios'
import { ElMessage } from 'element-plus'
import { useNotificationsStore } from '../../stores/notifications'
import { useRequestLogStore } from '../../stores/requestLog'
import { useAuthStore } from '../../stores/auth'
import { clearToken } from '../token'
import { ERR, describeErrorCode } from '../../utils/errorCodes'
import { withDiagHint } from '../../utils/diagHints'
import { logWarn } from '../../utils/logger'
import { instance, unregisterInflight, type TimedConfig } from './instance'
import { readTraceId } from './trace'
import { isPlainObject, loggableBody, loggableHeaders, sanitizeUrl } from './sanitize'
import { isBareResponse, type ApiEnvelope } from './envelope'
import { releaseLoggingOut, tryAcquireLoggingOut } from './loggingOut'

instance.interceptors.response.use(
  (response) => {
    const cfg = response.config as TimedConfig
    unregisterInflight(cfg)
    const startAt = cfg._startAt ?? Date.now()
    const duration = Date.now() - startAt

    const traceId = readTraceId(response.headers)
    const body = response.data
    let responseData = body
    // P1-1：后端 router 层已把 /api/* 的 2xx JSON 统一包成 ApiResponse 信封
    // （gateway-api/src/web/middleware.rs::envelope），此处**只按信封解包**，
    // 不再靠「猜 body 里有没有 code/data/message」来决定是否解包——
    // 那种猜法会让某个 handler 漏改造后静默返回整个信封对象。
    // 非 JSON 对象（text/plain、CSV/blob、空响应）与豁免路径（见 isBareResponse）直通。
    if (isPlainObject(body) && !isBareResponse(cfg.url)) {
      const wrapped = body as unknown as ApiEnvelope
      if (typeof wrapped.code !== 'number') {
        // 理论上不存在（后端中间件已兜底），真出现说明契约被绕过，原样透出便于定位
        if (import.meta.env.DEV) {
          logWarn('api:envelope', '响应缺少 ApiResponse 信封:', sanitizeUrl(cfg.url), body)
        }
      } else if (wrapped.code !== 0) {
        // 后端 message 有值时原样透出；为空则用错误码字典兜底，
        // 避免用户只看到 `biz:8000` 这类裸码。
        const message = describeErrorCode(wrapped.code, wrapped.message)
        useRequestLogStore().pushLog({
          method: cfg.method?.toUpperCase() || 'GET',
          url: sanitizeUrl(cfg.url),
          status: `biz:${wrapped.code}`,
          duration, startTime: startAt, endTime: startAt + duration,
          requestData: loggableBody(cfg.method, cfg.url, cfg.data),
          error: message,
          traceId,
          requestHeaders: loggableHeaders(cfg.headers),
          responseHeaders: loggableHeaders(response.headers),
        })
        const notify = useNotificationsStore()
        notify.error(message, { code: wrapped.code, traceId })
        // traceId 挂到 Error 上：页面/组合式函数在上下文提示里也能带上它
        const bizError = new Error(message) as Error & { traceId?: string }
        bizError.traceId = traceId
        return Promise.reject(bizError)
      } else {
        responseData = wrapped.data
        response.data = wrapped.data
      }
    }

    useRequestLogStore().pushLog({
      method: cfg.method?.toUpperCase() || 'GET',
      url: sanitizeUrl(cfg.url),
      status: response.status,
      duration, startTime: startAt, endTime: startAt + duration,
      requestData: loggableBody(cfg.method, cfg.url, cfg.data),
      responseData,
      traceId,
      requestHeaders: loggableHeaders(cfg.headers),
      responseHeaders: loggableHeaders(response.headers),
    })
    return response
  },
  (error) => {
    const cfg = (error.config || {}) as TimedConfig
    unregisterInflight(cfg)
    const startAt = cfg._startAt ?? Date.now()
    const duration = Date.now() - startAt

    // 用户主动发起的写操作（POST/PUT/DELETE/PATCH）失败时，调用方页面/组合式函数会给出
    // 带动作语境的提示（如「保存自愈配置失败」）；全局再弹一条就是两条 Toast 叠在一起。
    // 故写操作只在通知中心留痕、不弹窗，保证用户只看到一条有上下文的提示。
    // GET 多为轮询/加载，用户没有主动发起，也没有人负责提示，仍由全局弹。
    // 例外（401 鉴权 / 4021 配额 / 429 限流）保持全局提示：它们是跨页面的全局状态，需要立刻可见。
    const isMutation = (cfg.method ?? 'get').toLowerCase() !== 'get'

    // 主动取消：切网关 abortAllInflight / 组件卸载 AbortController.abort()
    // 不弹通知、不写错误日志
    if (axios.isCancel(error) || error.code === 'ERR_CANCELED') {
      return Promise.reject(error)
    }

    if (error.response) {
      const status = error.response.status
      const body = error.response.data
      const traceId = readTraceId(error.response.headers)
      const msg = withDiagHint(body?.error || body?.message || error.message)
      // 后端用 ApiResponse{code,message} 返回 4xx/5xx（如配置/用户/点位校验失败）时，
      // 页面级 catch 常直接展示 `e.message`；axios 原始 message 是英文
      // 「Request failed with status code 400」，会把后端的中文原因吃掉。
      // 这里把已归一化的文案回写到 error.message，让错误分支与业务分支（code!==0）一致。
      if (msg && error instanceof Error && error.message !== msg) {
        try {
          error.message = msg
        } catch {
          /* 少数被冻结的 Error 对象忽略即可 */
        }
      }
      const isLoginApi = (cfg.url || '').includes('/api/auth/login')
      // #11: 改密接口的 401 表示"当前密码错误"，须透传给业务层提示，
      // 不能触发全局登出流程（否则用户看到的是"鉴权失败，请重新登录"，指向错误方向）。
      const isPasswordApi = (cfg.url || '').includes('/api/auth/password')
      const isLoginPage = window.location.pathname === '/login'
      if (status === 401) {
        if (isLoginApi || isPasswordApi) {
          // 透传给业务层
        } else {
          // Bug 2.10: 用 tryAcquireLoggingOut 替代 boolean set，
          // 多个并发 401 中**仅第一个**走"弹通知+跳登录"路径，
          // 其余静默丢弃，杜绝多次弹窗 / 多次跳登录。
          const shouldFire = tryAcquireLoggingOut()
          if (shouldFire) {
            clearToken()
            try {
              useAuthStore().$patch({ loggedIn: false, token: '' })
            } catch { /* pinia 未激活 */ }
            if (!isLoginPage) {
              const notify = useNotificationsStore()
              notify.error('鉴权失败，请重新登录', { code: ERR.UNAUTHORIZED, traceId })
              // Bug 179: 延迟到跳登录流程结束后再释放登出门。原实现紧接着同步 release，
              // 使 ref-count 在同一同步批次内立刻回到 0，导致并发 401 每一个都判定为
              // “第一个”而重复弹窗/跳登录。改为动态导入 + replace 完成后再释放，保证
              // 流程进行期间并发 401 走静默分支；无论成功/失败都释放，严格配对。
              void import('../../router')
                .then(({ default: router }) => {
                  router.replace({ name: 'Login' }).catch(() => {
                    window.location.href = '/login'
                  })
                })
                .finally(() => releaseLoggingOut())
            } else {
              // 已在登录页：无需跳转，立即释放
              releaseLoggingOut()
            }
          } else {
            // Bug 179: 静默丢弃分支——tryAcquire 已自增计数，必须立即 release 严格配对，
            // 否则计数永不归零，token 再次过期时也不会再跳登录（直到下次登录
            // resetLoggingOut 才复位）。
            releaseLoggingOut()
          }
        }
      } else if (
        body?.code === ERR.QUOTA_EXCEEDED ||
        body?.code === ERR.LICENSE_REQUIRED ||
        body?.code === ERR.LICENSE_INVALID
      ) {
        // v2.0 授权类拒绝（HTTP 400/403）：
        //  - 4021 授权有效但档位配额不够 → 引导升级档位
        //  - 7001 从未导入授权 → 引导导入 .lic
        //  - 7003 授权已失效（过期/设备码不符/被篡改）→ 引导重新导入或续期
        // 三者都是**跨页面的全局状态**，必须立刻可见，故不受 isMutation 静音约束。
        // 后端返回面向客户的中文 message（如"当前档位支持最多 1500 点位数，当前已 2000"），
        // 原样透出 + 授权编号增强定位。
        const detail = describeErrorCode(body.code, body?.message)
        void import('../../stores/tier')
          .then(({ useTierStore }) => {
            const licId = useTierStore().licenseState?.license_id
            useNotificationsStore().error(
              licId ? `${detail}（授权编号: ${licId}）` : detail,
              { code: body.code, traceId },
            )
          })
          .catch(() => {
            useNotificationsStore().error(detail, { code: body.code, traceId })
          })
      } else if (status === 429) {
        ElMessage.warning('请求过于频繁，请稍后重试')
      } else {
        // code 传 HTTP 状态码：通知中心按 100–599 区间识别并给出人话解释 + 排查建议
        // （业务码 4021/8000 都在区间外，不会与 ApiResponse.code 混淆）
        useNotificationsStore().error(`请求失败(${status}): ${msg}`, { code: status, silent: isMutation, traceId })
      }
      useRequestLogStore().pushLog({
        method: cfg.method?.toUpperCase() || 'GET',
        url: sanitizeUrl(cfg.url),
        status,
        duration, startTime: startAt, endTime: startAt + duration,
        requestData: loggableBody(cfg.method, cfg.url, cfg.data),
        error: msg,
        traceId,
        requestHeaders: loggableHeaders(cfg.headers),
        responseHeaders: loggableHeaders(error.response.headers),
      })
    } else {
      const rawErr = typeof error === 'string' ? error : error.message || String(error)
      const errMsg = withDiagHint(rawErr)
      // 网络层失败拿不到后端响应，自然没有 traceId——给出现场可执行的自检项，
      // 避免用户只看到一句"网络错误"无从下手
      const selfCheck = '自检：确认网关服务已启动、访问地址与端口正确、本机与网关网络连通'
      useNotificationsStore().error(`网络错误: ${errMsg}；${selfCheck}`, { silent: isMutation })
      useRequestLogStore().pushLog({
        method: cfg.method?.toUpperCase() || 'GET',
        url: sanitizeUrl(cfg.url),
        status: 'NETWORK',
        duration, startTime: startAt, endTime: startAt + duration,
        requestData: loggableBody(cfg.method, cfg.url, cfg.data),
        error: errMsg,
        // 网络层失败没有响应头，但请求头仍在（重放时同样需要）
        requestHeaders: loggableHeaders(cfg.headers),
      })
    }
    return Promise.reject(error)
  },
)
