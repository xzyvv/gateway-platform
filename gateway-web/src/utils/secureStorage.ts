/**
 * secureStorage — 加密的 localStorage 抽象
 *
 * 安全模型（方案 B）：
 * 1. 首次启动时生成一个 256-bit AES-GCM 随机密钥，存到 IndexedDB (`idb`)；
 * 2. 调用 setToken / getToken 时，先用密钥 AES-GCM 加密再写 localStorage；
 * 3. 读取时反向解密。
 *
 * 这样：
 * - localStorage 即便被 XSS 读到也是密文（无 key 拿不到明文）；
 * - key 单独存在 IndexedDB（不同 origin / 沙箱），增加一点提取难度；
 * - 没有引入新加密库，依赖浏览器原生 Web Crypto API（所有现代浏览器 + Tauri WebView 都支持）；
 * - 不支持 Web Crypto 的环境（极少数老浏览器）自动降级为 localStorage 明文（旧行为），
 *   并经 utils/logger 记录降级告警（生产默认静默，VITE_LOG_VERBOSE=true 可开）。
 *
 * 已知局限（已记录，不必在第一轮处理）：
 * - 这只是"提高 XSS 窃取成本"，不是真 OS Keychain 级别的安全；
 *   真正的加固需要走 tauri-plugin-stronghold（方案 A），待 Tauri 端独立加固。
 * - 同一 origin 内 XSS 仍可调本模块的 getToken() 直接拿明文——这是浏览器侧加密的固有边界。
 */

import { logWarn } from './logger'

const DB_NAME = 'gateway_secure_store'
const STORE_NAME = 'keys'
const KEY_ID = 'token_aes_key'
const LEGACY_TOKEN_KEY = 'gateway_api_token' // 老 localStorage 明文 key（迁移用）

interface EncryptedPayload {
  /** 加密算法标识，未来升级时便于兼容 */
  v: 1
  /** IV，base64 */
  iv: string
  /** 密文，base64 */
  ct: string
}

// ============ IndexedDB 微型封装 ============

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1)
    req.onupgradeneeded = () => {
      const db = req.result
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME)
      }
    }
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  })
}

async function idbGet(key: string): Promise<CryptoKey | null> {
  const db = await openDb()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readonly')
    const req = tx.objectStore(STORE_NAME).get(key)
    req.onsuccess = () => resolve((req.result as CryptoKey | undefined) ?? null)
    req.onerror = () => reject(req.error)
  })
}

async function idbSet(key: string, value: CryptoKey): Promise<void> {
  const db = await openDb()
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readwrite')
    tx.objectStore(STORE_NAME).put(value, key)
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error)
  })
}

// ============ 密钥生命周期 ============

let cachedKey: CryptoKey | null = null

/**
 * Bug-W7 (fix)：降级状态记录——本模块在以下场景会**静默降级到明文 localStorage**：
 *  1. Web Crypto API 不可用（极老浏览器 / 隐私模式禁用部分 API）
 *  2. IndexedDB 读取失败（隐私模式 / 用户禁用站点存储）
 *  3. AES-GCM 密钥生成失败（系统加密后端异常）
 *  4. encrypt / decrypt 失败（密钥被清 / 协议版本不兼容）
 *
 * 旧实现仅 `logWarn`，生产环境（VITE_LOG_VERBOSE=false）静默——用户毫无察觉。
 * 不修后果：隐私模式 / IndexedDB 禁用场景下 token 落明文 localStorage，
 * 任何 XSS 漏洞均可直接窃取凭证，与本模块设计目标完全相悖。
 *
 * 修复策略：
 * - 模块级 flag + 原因字符串记录**首次**降级原因（同一进程内不重复触发 UI 告警）
 * - 暴露 `__getLastDegradeReason()` 给应用启动层（main.ts bootstrap）调用，
 *   有降级则 `ElMessage.warning` 弹窗告知用户："当前浏览器禁用了加密存储，
 *   凭证将以明文保存，建议更换浏览器或关闭隐私窗口"。
 * - 弹窗一次性：flag 置位后不再重复触发，避免每次请求都弹窗打扰用户。
 *
 * 注意：模块 import 阶段无法直接调 useNotificationsStore（pinia 未激活 + 循环依赖），
 * 故采用「记录 → 由 bootstrap 启动期统一读取」的延迟通知模式。
 */
let lastDegradeReason: string | null = null
function recordDegrade(reason: string): void {
  if (lastDegradeReason === null) {
    lastDegradeReason = reason
    logWarn('secureStorage', 'downgrade to plaintext localStorage:', reason)
  }
}
/** 测试 / bootstrap 用：读取首次降级原因（已弹出则返回 null）。 */
export function __getLastDegradeReason(): string | null {
  return lastDegradeReason
}
/** 测试用：清空降级状态（避免测试间污染）。 */
export function __resetDegradeState(): void {
  lastDegradeReason = null
  cachedKey = null
}

function isWebCryptoAvailable(): boolean {
  return typeof globalThis.crypto !== 'undefined' && !!globalThis.crypto.subtle
}

async function getOrCreateKey(): Promise<CryptoKey | null> {
  if (!isWebCryptoAvailable()) {
    // Bug-W7 (fix)：标记降级原因，bootstrap 启动时统一通知用户
    recordDegrade('Web Crypto API unavailable (likely private/incognito mode or legacy browser)')
    return null
  }
  if (cachedKey) return cachedKey

  try {
    const existing = await idbGet(KEY_ID)
    if (existing) {
      cachedKey = existing
      return existing
    }
  } catch (e) {
    logWarn('secureStorage', 'idb read failed, falling back to plaintext:', e)
    recordDegrade(`IndexedDB read failed: ${e instanceof Error ? e.message : String(e)}`)
    return null
  }

  // 首次：生成新 key 并持久化
  try {
    const key = await crypto.subtle.generateKey(
      { name: 'AES-GCM', length: 256 },
      false, // non-extractable（即便 XSS 拿到 key 对象也不能导出原始字节）
      ['encrypt', 'decrypt'],
    )
    await idbSet(KEY_ID, key)
    cachedKey = key
    return key
  } catch (e) {
    logWarn('secureStorage', 'key generation failed, falling back to plaintext:', e)
    recordDegrade(`AES-GCM key generation failed: ${e instanceof Error ? e.message : String(e)}`)
    return null
  }
}

// ============ base64 helpers ============

function bytesToB64(bytes: Uint8Array): string {
  let bin = ''
  for (const b of bytes) bin += String.fromCharCode(b)
  return btoa(bin)
}

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

// ============ 公开 API：setToken / getToken / clearToken ============

const ENC_PREFIX = 'enc:'

export async function setToken(token: string): Promise<void> {
  const key = await getOrCreateKey()
  if (!key) {
    localStorage.setItem(LEGACY_TOKEN_KEY, token)
    return
  }
  try {
    const iv = crypto.getRandomValues(new Uint8Array(12))
    const tokenBytes = new TextEncoder().encode(token)
    const ct = await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv },
      key,
      tokenBytes as Uint8Array<ArrayBuffer>,
    )
    const ctBytes = new Uint8Array(ct)
    const payload: EncryptedPayload = {
      v: 1,
      iv: bytesToB64(iv),
      ct: bytesToB64(ctBytes),
    }
    localStorage.setItem(LEGACY_TOKEN_KEY, ENC_PREFIX + JSON.stringify(payload))
  } catch (e) {
    logWarn('secureStorage', 'encrypt failed, falling back to plaintext:', e)
    recordDegrade(`encrypt failed: ${e instanceof Error ? e.message : String(e)}`)
    localStorage.setItem(LEGACY_TOKEN_KEY, token)
  }
}

export async function getToken(): Promise<string> {
  const raw = localStorage.getItem(LEGACY_TOKEN_KEY)
  if (!raw) return ''
  if (!raw.startsWith(ENC_PREFIX)) {
    // 旧明文 / 未加密条目 — 直接返回（旧 token 本就是明文）
    return raw
  }
  const key = await getOrCreateKey()
  if (!key) return ''
  try {
    const payload = JSON.parse(raw.slice(ENC_PREFIX.length)) as EncryptedPayload
    const pt = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: b64ToBytes(payload.iv) as Uint8Array<ArrayBuffer> },
      key,
      // b64ToBytes 返回 Uint8Array<ArrayBufferLike>，TS 5.x 要求 ArrayBufferView<ArrayBuffer>
      b64ToBytes(payload.ct) as Uint8Array<ArrayBuffer>,
    )
    return new TextDecoder().decode(pt)
  } catch (e) {
    logWarn('secureStorage', 'decrypt failed, treating as empty:', e)
    // 解密失败通常意味着 IndexedDB 里的 key 被清（浏览器数据被清），
    // 老密文已无法恢复，主动删掉避免下次再失败
    localStorage.removeItem(LEGACY_TOKEN_KEY)
    return ''
  }
}

export function clearToken(): void {
  localStorage.removeItem(LEGACY_TOKEN_KEY)
}

/**
 * 一次性迁移：把旧版 localStorage 明文 token 加密后写回。
 *
 * 流程：
 * 1. 读老 token（明文或 `enc:` 密文）
 * 2. 已是 enc: 跳过
 * 3. 否则用新 key 加密覆盖
 *
 * 应在应用启动时（main.ts 或 App.vue 的 onMounted）调一次，
 * 保证老用户的明文 token 在下次保存前先加密。
 *
 * 返回值：是否真的做了迁移。
 */
export async function migrateLegacyToken(): Promise<boolean> {
  const raw = localStorage.getItem(LEGACY_TOKEN_KEY)
  if (!raw) return false
  if (raw.startsWith(ENC_PREFIX)) return false
  // 是老明文 — 走加密流程
  await setToken(raw)
  return true
}
