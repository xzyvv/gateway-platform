import { defineStore } from 'pinia'
import { ref } from 'vue'
import { getToken, setToken, clearToken } from '@/api'
import * as authApi from '@/api/auth'
import { useSessions } from '../composables/system/useSessions'
import { markLoggingOut, resetLoggingOut } from '@/api/http'
import { type Role } from '../types/roles'
import { scopedGet, scopedRemove, scopedSet } from '../utils/gatewayStorage'

/// Bug 2.8: 三态身份验证状态机
///
/// - 'unknown' → 应用启动时初始态：localStorage 里有 token 但未向网关确认
/// - 'verifying' → 正在调用 whoami，UI 应展示 loading 而非"已登录"
/// - 'authenticated' → whoami 通过，token 有效
/// - 'anonymous' → 没有 token 或 token 已失效（401），需要重新登录
export type AuthStatus = 'unknown' | 'verifying' | 'authenticated' | 'anonymous'


export const useAuthStore = defineStore('auth', () => {
  // token ref 保留明文以便表单/调试用，但初始化用空串——
  // 实际拿明文 token 必须走 await getToken()，原因见 [utils/secureStorage.ts]。
  const token = ref('')
  // Bug 2.8: loggedIn 仅作路由守卫同步推断，不再表示"已登录"于 UI。
  // 真正的身份状态看 authStatus（三态机），依赖启动时调 validate()。
  //
  // A1 修复：`gateway_api_token` 可能是 `enc:` 密文，若 IndexedDB 中的密钥被清
  // （浏览器数据被清 / key 轮换），密文无法解密（getToken() 返回 ''），但 key 仍
  // 存在于 localStorage → 旧实现仅凭"key 存在"就置 loggedIn=true，路由守卫放行后
  // 才在后续接口 401。这里保留同步初值以便守卫首帧不误跳登录，但 validate() 会
  // 用真实可解密结果校正 loggedIn（见下方）。
  //
  // 注意：这里刻意**不**走 gatewayStorage 的命名空间前缀——token 的读写由
  // [utils/secureStorage.ts] 独占（`LEGACY_TOKEN_KEY`，含加密与迁移逻辑），
  // 只对「探测」加前缀会让写/读两端失配。token 的按网关隔离需与 secureStorage 一起改，
  // 属独立事项（W3-b）。
  const loggedIn = ref(localStorage.getItem('gateway_api_token') !== null)
  // 三态鉴权状态：'unknown' → 'verifying' → 'authenticated' | 'anonymous'
  // 仅依赖 localStorage 是不可靠的（token 可能过期），
  // 需等 validate() 调 whoami 后才能确认。
  const authStatus = ref<AuthStatus>('unknown')

  /** 当前用户名（whoami 后填入） */
  // ROI: 从 localStorage 恢复 UI 层展示，避免刷新时 username 闪空后再被 validate() 校正。
  // 仅用于首帧展示，不影响安全校验——validate() 仍会调 whoami 覆盖。
  const username = ref<string>(scopedGet('auth.ui.username') || '')

  /** 当前是否仍在使用默认密码，登录后由 Login.vue / whoami 写入 */
  const usingDefaultPassword = ref<boolean>(
    scopedGet('auth.ui.usingDefaultPassword') === 'true',
  )

  /** 从 localStorage 读安全风险项；解析失败按"无风险项"处理，避免整个 store 初始化失败 */
  function readSecurityWarnings(): string[] {
    try {
      const raw = scopedGet('auth.ui.securityWarnings')
      return raw ? (JSON.parse(raw) as string[]) : []
    } catch {
      return []
    }
  }

  /** 当前配置的安全风险项（默认凭据 / 匿名只读 / 开放 CORS / 全网卡监听），Login/Layout 提示用 */
  // 注意：`ref()` 不认工厂函数——传函数会被当成「值本身」，类型也对不上 string[]；
  // 想懒初始化必须先调函数再传结果（下面就是），这也是原写法报 TS2345 的原因。
  const securityWarnings = ref<string[]>(readSecurityWarnings())

  /**
   * 当前界面角色（仅用于按钮/菜单显隐，后端不做角色判定）。
   *
   * 生产模式由 whoami 返回的 `credential`/`scopes` 推导（见 refreshIdentity）：
   * 主 token → admin，API 密钥按写 scope 区分 operator/viewer。
   * 默认先按 admin，避免路由守卫首帧把有权用户误判为无权限；
   * whoami 成功后才校正。
   * 开发模式（VITE_USE_MOCK=true）由 mock 用户设定，供演示角色差异。
   */
  const roleId = ref<Role['id']>('admin')

  /** whoami 返回的凭证类型（旧版本网关缺失时为 'unknown'） */
  const credential = ref<'primary' | 'api_key' | 'unknown'>('unknown')
  /** whoami 返回的 API 密钥 scope（主 token 为 ['*']） */
  const scopes = ref<string[]>([])

  /** 按凭证真实能力推导界面角色（不涉及后端授权） */
  function deriveRoleFromCredential(c: 'primary' | 'api_key' | 'unknown', s: string[]): Role['id'] {
    if (c === 'primary') return 'admin'
    if (s.some((x) => x === '*' || x === 'admin')) return 'admin'
    if (s.some((x) => x.endsWith(':write'))) return 'operator'
    return 'viewer'
  }

  /** 用户名密码登录：成功后 token 存入 localStorage，并记录登录会话 */
  async function login(
    // Bug-N3 (fix)：参数重命名为 `user` / `pass`——避免和模块作用域的
    // `username` 字段（Ref<string>）命名遮蔽。TypeScript 在 try 块内会
    // 把 `username.value` 解析为函数参数 `username: string`，导致 `.value`
    // 不存在（类型错误："Property 'value' does not exist on type 'string'"）。
    // 旧参数名沿用至今是历史遗留——本次修复顺手清理。
    user: string,
    pass: string,
  ): Promise<void> {
    // Bug-N3 (fix)：重构为 try/catch，确保失败路径也能正确重置 authStatus。
    //
    // 旧实现：登录 API 抛错时 authStatus 卡在 'verifying'，
    // → 任何后续跳转路由 / Layout 组件 / Sidebar 看到的都是 loading
    //   状态直到页面 reload；用户看到的"登录按钮还在转"导致死锁体验。
    //
    // 新实现：用 catch 在所有退出路径（含抛错、network 异常、401 等）
    // 把 authStatus 校准到正确的终态——成功 → 'authenticated'，抛错 → 'anonymous'。
    // 顺手把 token / loggedIn 等内部状态也重置（防止半成功状态泄漏，例如 token 已存但
    // resetLoggingOut 还没调用导致下一次 401 立刻登出）。
    authStatus.value = 'verifying'
    try {
      const resp = await authApi.login(user, pass)
      await setToken(resp.token)
      token.value = resp.token
      loggedIn.value = true
      // 新会话开始，清掉 401 单飞闸门
      resetLoggingOut()
      useSessions().markLogin()
      // 登录成功后立即 whoami 一次，拿到 username + 默认密码提醒
      await refreshIdentity().catch(() => {
        /* whoami 失败不影响主流程 */
      })
      authStatus.value = 'authenticated'
      // 登录成功后拉取 license 快照（§11.1：tier store 初始化来源）。
      // 动态 import 避免 http → stores/tier → api/license → http 的循环依赖。
      void import('./tier')
        .then(({ useTierStore }) => useTierStore().refreshFromLicense())
        .catch(() => {
          /* license 拉取失败不影响主流程 */
        })
    } catch (err) {
      // Bug-N3：失败路径重置 authStatus + 清掉已设的部分状态，
      // 防止后续组件看到不一致（卡 verifying 或 "loggedIn=true 但实际未登录"）。
      //
      // 注意：**不**清掉 localStorage 已存的旧 token——可能是用户在同一
      // localStorage 下已登录过另一个账号，密码错不应清掉那个 token。
      // 也不动 useSessions().markLogin()——只清 store 内部状态。
      token.value = ''
      loggedIn.value = false
      username.value = ''
      usingDefaultPassword.value = false
      securityWarnings.value = []
      clearUiFields()
      authStatus.value = 'anonymous'
      // 把原异常向上抛，调用方 Login.vue 仍能 ElMessage.error 展示
      throw err
    }
  }

  function logout() {
    clearToken()
    token.value = ''
    loggedIn.value = false
    authStatus.value = 'anonymous'
    username.value = ''
    usingDefaultPassword.value = false
    securityWarnings.value = []
    clearUiFields()
    // 主动登出也算进入登出流程
    markLoggingOut()
  }

  /**
   * 应用启动 / 路由守卫调：ping whoami 校验 token 有效性。
   * 401（token 失效）返回 false；网络错误/网关不可达返回 true（保留凭证并放行）；
   * 成功则同步身份信息并返回 true。
   *
   * Bug W1: 原实现把 401 与网络错误一律当作失效返回 false，导致一次网络抖动
   * 就被路由守卫 logout() 清掉 token。现仅 401 才清身份。
   */
  async function validate(): Promise<boolean> {
    if (!(await getToken())) {
      // A1 修复：token 不可用（不存在或密文无法解密）→ 同步纠正 loggedIn，
      // 避免路由守卫凭"key 存在"误判已登录后放行到受保护页面。
      loggedIn.value = false
      authStatus.value = 'anonymous'
      return false
    }
    // Bug 2.8: 设置 'verifying' 状态，UI 在 whoami 未返回前不应展示"已登录"菜单
    authStatus.value = 'verifying'
    try {
      await refreshIdentity()
      authStatus.value = 'authenticated'
      loggedIn.value = true
      return true
    } catch (err) {
      // Bug W1: 只有明确的 401（token 失效）才判定需要重新登录；
      // 网络错误 / 网关不可达 / 超时不应清凭证，否则网关重启或网络抖动一次
      // 就会把用户踢出并删除已存 token。此类情况保留身份、放行本次路由。
      const status = (err as { response?: { status?: number } } | undefined)?.response?.status
      if (status === 401) {
        username.value = ''
        usingDefaultPassword.value = false
        securityWarnings.value = []
        clearUiFields()
        authStatus.value = 'anonymous'
        // A1 修复：401 明确表示 token 失效，同步纠正 loggedIn
        loggedIn.value = false
        return false
      }
      // 网络错误：保留凭证但状态仍为 'authenticated'（上次验证过的未过期），允许路由不阻塞
      authStatus.value = 'authenticated'
      return true
    }
  }

  /** 调 whoami 同步 username + usingDefaultPassword + securityWarnings + 界面角色 */
  async function refreshIdentity(): Promise<void> {
    const me = await authApi.whoami()
    username.value = me.username
    usingDefaultPassword.value = me.using_default_password
    securityWarnings.value = me.security_warnings || []
    // 按凭证真实能力推导界面角色。dev mock 模式不覆盖（由 mock 用户决定演示角色）。
    credential.value = me.credential ?? 'unknown'
    scopes.value = me.scopes ?? []
    if (import.meta.env.VITE_USE_MOCK !== 'true' && credential.value !== 'unknown') {
      roleId.value = deriveRoleFromCredential(credential.value, scopes.value)
    }
    // ROI: whoami 成功后同步持久化 UI 字段，供下次刷新直接恢复。
    persistUiFields()
  }

  /** ROI: UI 层字段持久化到 localStorage，供下次刷新首帧直接展示（不影响安全校验）。 */
  function persistUiFields() {
    scopedSet('auth.ui.username', username.value || '')
    scopedSet('auth.ui.usingDefaultPassword', String(usingDefaultPassword.value))
    try {
      scopedSet('auth.ui.securityWarnings', JSON.stringify(securityWarnings.value))
    } catch {
      /* 序列化失败忽略 */
    }
  }

  /** ROI: 清除 UI 层持久化字段——登出 / 401 / 登录失败时调用。 */
  function clearUiFields() {
    scopedRemove('auth.ui.username')
    scopedRemove('auth.ui.usingDefaultPassword')
    scopedRemove('auth.ui.securityWarnings')
  }

  return { token, loggedIn, authStatus, username, usingDefaultPassword, securityWarnings, roleId, credential, scopes, login, logout, validate, refreshIdentity }
})
