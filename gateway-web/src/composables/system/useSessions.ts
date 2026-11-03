/**
 * 登录会话管理（前端模拟）
 * 后端目前只支持单 token，这里在前端模拟"会话列表"概念：
 * - 记录本次登录时间、来源 IP（从 X-Forwarded-For 或未知）
 * - 支持查看活跃会话、强制下线（即清 token）
 */
import { ref } from 'vue'
import { getToken, clearToken } from '@/api/index'
import { useAuthStore } from '@/stores/auth'
import { scopedGet, scopedRemove, scopedSet } from '@/utils/gatewayStorage'

export interface LoginSession {
  id: string
  loginAt: number
  lastActiveAt: number
  ip: string
  userAgent: string
  current: boolean
}

const STORAGE_KEY = 'gw_sessions'
const MAX_SESSIONS = 20

function load(): LoginSession[] {
  try {
    const raw = scopedGet(STORAGE_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    // 字段完整性校验：脏数据（缺 id/loginAt）一律丢弃
    return parsed.filter((s: unknown): s is LoginSession =>
      !!s && typeof s === 'object' && typeof (s as LoginSession).id === 'string' && typeof (s as LoginSession).loginAt === 'number'
    ).map((s: LoginSession) => ({
      ...s,
      current: typeof s.current === 'boolean' ? s.current : false,
      ip: typeof s.ip === 'string' ? s.ip : '127.0.0.1',
      userAgent: typeof s.userAgent === 'string' ? s.userAgent.slice(0, 120) : '',
      lastActiveAt: typeof s.lastActiveAt === 'number' ? s.lastActiveAt : s.loginAt,
    }))
  } catch { return [] }
}

function save(sessions: LoginSession[]) {
  try { scopedSet(STORAGE_KEY, JSON.stringify(sessions)) } catch { /* ignore */ }
}

// @singleton: 登录会话列表需跨页面共享（路由守卫 + Sessions 管理页），有意使用模块级 ref
// 持久化到 localStorage，后续如需 devtools 追踪可迁移到 Pinia store
// eslint-disable-next-line no-restricted-syntax
const sessions = ref<LoginSession[]>(load())

function currentSessionId(): string {
  return scopedGet('gw_current_session') || ''
}

export function useSessions() {
  /** 记录一次新登录 */
  function markLogin(ip?: string) {
    const id = `sess_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`
    const now = Date.now()
    // 旧的 current 置 false
    sessions.value.forEach((s) => { s.current = false })
    const sess: LoginSession = {
      id,
      loginAt: now,
      lastActiveAt: now,
      ip: ip || '127.0.0.1',
      userAgent: navigator.userAgent.slice(0, 120),
      current: true,
    }
    sessions.value.push(sess)
    // 截断到 MAX_SESSIONS：保留最新的 N 条会话，丢弃最早的非 current 会话
    if (sessions.value.length > MAX_SESSIONS) {
      const overflow = sessions.value.length - MAX_SESSIONS
      // 优先删除非 current 的（保留所有 current，避免被强制下线）
      const dropIdx = new Set<number>()
      for (let i = 0; i < sessions.value.length && dropIdx.size < overflow; i++) {
        if (!sessions.value[i]!.current) dropIdx.add(i)
      }
      // 仍不够：按 loginAt 从最旧到最新补删（保留最新 MAX_SESSIONS-1 条）
      if (dropIdx.size < overflow) {
        const sortedByLogin = sessions.value
          .map((s, i) => ({ s, i }))
          .filter(({ s }) => !s.current) // 已 current 全保留
          .sort((a, b) => a.s.loginAt - b.s.loginAt)
        for (const { i } of sortedByLogin) {
          if (dropIdx.size >= overflow) break
          dropIdx.add(i)
        }
      }
      sessions.value = sessions.value.filter((_, i) => !dropIdx.has(i))
    }
    scopedSet('gw_current_session', id)
    save(sessions.value)
    return sess
  }

  /** 刷新当前会话的活跃时间 */
  function touch() {
    const id = currentSessionId()
    const s = sessions.value.find((x) => x.id === id)
    if (s) {
      s.lastActiveAt = Date.now()
      save(sessions.value)
    }
  }

  /** 强制下线某个会话 */
  function revoke(id: string) {
    const s = sessions.value.find((x) => x.id === id)
    const isCurrent = !!s?.current
    sessions.value = sessions.value.filter((x) => x.id !== id)
    save(sessions.value)
    if (isCurrent) {
      // Bug 180: 下线「当前会话」必须真正登出并跳登录页。原实现仅 clearToken()：
      // 内存中的 auth store 仍认为已登录、当前页面也不会跳转，用户可继续操作
      // （请求会 401 但体验错误）。补齐清 store + 清当前会话标记 + 跳登录。
      clearToken()
      try {
        useAuthStore().$patch({ loggedIn: false, token: '' })
      } catch { /* pinia 未激活 */ }
      if (scopedGet('gw_current_session') === id) {
        scopedRemove('gw_current_session')
      }
      if (window.location.pathname !== '/login') {
        void import('@/router').then(({ default: router }) => {
          router.replace({ name: 'Login' }).catch(() => {
            window.location.href = '/login'
          })
        })
      }
    }
  }

  /** 获取会话列表 */
  function list(): LoginSession[] {
    return sessions.value
  }

  /** 检查当前 token 是否仍有效（是否需要跳登录） */
  async function isLoggedIn(): Promise<boolean> {
    return !!(await getToken())
  }

  return {
    // state
    sessions,
    // methods
    markLogin,
    touch,
    revoke,
    list,
    isLoggedIn,
  }
}



