// === W-4 / #15 修复：请求日志敏感字段脱敏 ===
// 问题：旧实现在响应拦截器里把 cfg.data 原样写进 requestLog，且仅在白名单路径
// （/api/auth/*）才脱敏；而 POST /api/config（含 MQTT 密码 / TLS 私钥 / webhook 密钥）、
// 证书上传等路径不在白名单，明文凭据会进入 requestLog store，用户截图或导出诊断包即外泄。
//
// #15 修复：改为对所有请求体统一走 sanitizeForLog（按字段名递归脱敏），并对二进制体
// （FormData/Blob/ArrayBuffer）直接省略，避免私钥/证书内容落日志。
//
// 维护说明：新增"含凭据"的字段名特征时，把片段加到 SENSITIVE_KEY_PATTERNS。
// 任何拼入日志的 error 字段也走 sanitizeForLog，保证不出明文。
//
// W-5 修复：匹配方式由「精确 key 白名单」改为「**小写子串**命中」。
// 旧实现是 SENSITIVE_KEYS.includes(k)，命名变体全部漏网——pwd / passwd /
// passphrase / apiKey / client_secret / privateKey / refreshToken /
// Authorization / credential 都会原样落进 requestLog，导出诊断包即外泄。
// 子串匹配会带来少量"误伤"（如名为 keys / keywords 的字段），但对日志来说
// **多脱敏**远优于漏脱敏，且不影响任何真实请求体。

/** 敏感字段名的**小写子串**特征（命中即整值替换为 '***'） */
const SENSITIVE_KEY_PATTERNS: ReadonlyArray<string> = [
  'pass',       // password / passwd / passphrase / new_password / old_password
  'pwd',        // 缩写写法
  'secret',     // secret / client_secret
  'token',      // token / access_token / refreshToken / api_token
  'key',        // api_key / apiKey / private_key / signing_key
  'auth',       // Authorization / authToken / oauth
  'credential', // credential / credentials
  'cookie',     // Cookie / Set-Cookie（会话凭据）
  'session',    // session / sessionid（会话凭据）
]

/** 字段名是否属于凭据类（大小写不敏感的子串匹配，W-5） */
function isSensitiveKey(key: string): boolean {
  const lower = key.toLowerCase()
  return SENSITIVE_KEY_PATTERNS.some((p) => lower.includes(p))
}

/** 简单类型判定，避免处理循环引用 / Date / Map / Set */
export function isPlainObject(v: unknown): v is Record<string, unknown> {
  if (v === null || typeof v !== 'object') return false
  const proto = Object.getPrototypeOf(v)
  return proto === Object.prototype || proto === null
}

/** 深拷贝并把敏感字段名（见 isSensitiveKey）对应的值替换为 '***'。非对象原样返回。
 *  可见性：请求日志（本模块内部）与诊断包配置快照（useDiagnosticExport）共用；
 *  两者都是"会被转发出去"的产物，必须保持同一份脱敏口径。 */
export function sanitizeForLog<T>(value: T, depth = 0): T {
  // 防递归栈爆 + 跳过非 plain object
  if (depth > 8) return value
  if (Array.isArray(value)) {
    return value.map((v) => sanitizeForLog(v, depth + 1)) as unknown as T
  }
  if (isPlainObject(value)) {
    const out: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(value)) {
      if (isSensitiveKey(k)) {
        out[k] = '***'
      } else {
        out[k] = sanitizeForLog(v, depth + 1)
      }
    }
    return out as T
  }
  return value
}

/**
 * URL 脱敏（W-4 修复）：请求日志里的 url 原样落进 requestLog，而 query 里可能带
 * 凭据（`?token=...`、`?password=...`、`?api_key=...`），导出诊断包或截图即外泄。
 * 这里把 query 中**字段名命中 isSensitiveKey** 的参数值替换为 `***`；路径与非敏感
 * 参数保留（排障需要看到打到哪个接口、带了哪些正常参数）。
 *
 * 可见性：**仅供测试**与本模块内部使用，业务代码请勿引用。
 */
export function sanitizeUrl(url: string | undefined): string {
  if (!url) return url ?? ''
  const q = url.indexOf('?')
  if (q < 0) return url
  const path = url.slice(0, q)
  const redactedQuery = url
    .slice(q + 1)
    .split('&')
    .map((pair) => {
      if (!pair) return pair
      const eq = pair.indexOf('=')
      if (eq < 0) return pair
      const name = pair.slice(0, eq)
      return isSensitiveKey(name) ? `${name}=***` : pair
    })
    .join('&')
  return `${path}?${redactedQuery}`
}

/** 用于日志记录的统一入口：所有请求体一律脱敏后再交给 store */
export function loggableBody(_method: string | undefined, _url: string | undefined, data: unknown): unknown {
  if (data === undefined || data === null) return data
  // 二进制 / 表单体（证书上传等）无法逐字段脱敏，直接省略，避免私钥/证书内容落日志
  if (typeof FormData !== 'undefined' && data instanceof FormData) return '[form-data omitted]'
  if (typeof Blob !== 'undefined' && data instanceof Blob) return '[blob omitted]'
  if (typeof ArrayBuffer !== 'undefined' && data instanceof ArrayBuffer) return '[binary omitted]'
  return sanitizeForLog(data)
}

/**
 * 请求/响应头 → 日志可用的 [{ name, value }] 列表（P1-12）。
 *
 * 修复前不记录任何头，导致导出的 HAR `headers: []` 恒空、「复制为 cURL」
 * 只能给方法+URL+Body——带鉴权的接口在终端根本没法重放。
 *
 * 脱敏规则：
 *   - 头名命中 isSensitiveKey（authorization / cookie / token / key …）→ 值记为 '***'，
 *     **不落明文**。诊断包会被邮件/IM 转发，凭据不能随之出去。
 *   - 其余头原样保留：Content-Type / Accept / x-request-id 等排障时都要看。
 *   - 长度上限：单个头值截断，避免异常长头撑爆日志。
 *
 * 注意：Authorization 被脱敏后，用户仍可在终端用「自己的 token」重放——
 * 报告给出完整的 URL + 方法 + 其它头 + Body，只需自行补 Authorization 即可。
 * 这是刻意的取舍：不落凭据，但让「缺哪一头」一目了然。
 *
 * 可见性：**仅供测试**与本模块内部使用，业务代码请勿引用。
 */
export function loggableHeaders(headers: unknown): Array<{ name: string; value: string }> {
  const out: Array<{ name: string; value: string }> = []
  if (!headers || typeof headers !== 'object') return out
  const raw = headers as {
    toJSON?: () => unknown
    entries?: () => Iterable<[string, unknown]>
  }
  let entries: Array<[string, unknown]> = []
  if (typeof raw.entries === 'function') {
    // AxiosHeaders：标准可迭代
    entries = Array.from(raw.entries())
  } else if (typeof raw.toJSON === 'function') {
    const obj = raw.toJSON()
    if (obj && typeof obj === 'object') entries = Object.entries(obj as Record<string, unknown>)
  } else {
    // 普通对象（测试里构造的字面量、以及部分自定义适配器）：直接取自有可枚举键
    entries = Object.entries(headers as Record<string, unknown>)
  }
  for (const [name, value] of entries) {
    if (!name) continue
    const str = value == null ? '' : String(value)
    const clipped = str.length > 512 ? `${str.slice(0, 512)}...(truncated)` : str
    out.push({ name, value: isSensitiveKey(name) ? '***' : clipped })
  }
  return out
}
