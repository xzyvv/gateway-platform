/**
 * E2E 网关夹具：启动一个"空配置"的 gatewayd 实例供 Playwright 冒烟。
 *
 * 设计要点：
 * - 跨平台：纯 Node API（child_process/fs/os），Windows 本地与 ubuntu CI 同一份脚本
 * - 隔离：配置与运行数据全部落在 os.tmpdir()/gateway-e2e-<pid>/，退出整目录清理
 * - 就绪判定：轮询 /health/live 直到 200（超时 fail fast），不靠固定 sleep
 * - 生命周期：脚本作为前台进程常驻，收到 SIGINT/SIGTERM 或父进程断开时
 *   终止 gatewayd 并清理，适配 Playwright webServer 的启动/回收模型
 *
 * 二进制查找顺序：环境变量 GATEWAYD_BIN（兼容旧名 GATEWAY_CORE_BIN）→ workspace target/debug 下
 * （本地与 CI 均为 cargo build 产物；冒烟用 debug 即可，省去 release 编译时间）
 */
import { spawn } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
// gateway-web/scripts/e2e -> workspace 根（gateway-platform/）
const WORKSPACE_ROOT = resolve(HERE, '..', '..', '..')

// P2-3：E2E 网关独立端口，默认 18080（不占用现场/本机真实网关的 8080）
const GATEWAY_PORT = Number(process.env.GATEWAY_E2E_PORT ?? 18080)
const GATEWAY_ADDR = `127.0.0.1:${GATEWAY_PORT}`
const HEALTH_URL = process.env.GATEWAY_HEALTH_URL || `http://${GATEWAY_ADDR}/health/live`
const READY_TIMEOUT_MS = 30_000
const POLL_INTERVAL_MS = 300

/** 空配置：无总线/点位/MQTT，允许匿名读；登录走默认 admin/admin（prepare_web_config 生成 token） */
const EMPTY_CONFIG = {
  buses: [],
  points: [],
  mqtt: {},
  web: { allow_anon_read: true, listen_addr: GATEWAY_ADDR },
}

function resolveBinary() {
  const fromEnv = process.env.GATEWAYD_BIN || process.env.GATEWAY_CORE_BIN
  if (fromEnv) {
    if (!existsSync(fromEnv)) throw new Error(`GATEWAYD_BIN 指向的文件不存在: ${fromEnv}`)
    return fromEnv
  }
  const exe = process.platform === 'win32' ? 'gatewayd.exe' : 'gatewayd'
  const candidates = [
    join(WORKSPACE_ROOT, 'target', 'debug', exe),
    join(WORKSPACE_ROOT, 'gatewayd', 'target', 'debug', exe),
  ]
  const found = candidates.find((p) => existsSync(p))
  if (!found) {
    throw new Error(
      `找不到 gatewayd 二进制（已尝试: ${candidates.join(', ')}）。\n` +
        '请先在 workspace 根执行 cargo build，或设置 GATEWAYD_BIN 环境变量。',
    )
  }
  return found
}

function waitForHealthy() {
  const deadline = Date.now() + READY_TIMEOUT_MS
  return new Promise((resolvePromise, rejectPromise) => {
    const tick = async () => {
      try {
        const resp = await fetch(HEALTH_URL)
        if (resp.ok) {
          resolvePromise()
          return
        }
      } catch {
        // 连接拒绝是启动中的正常状态，继续轮询
      }
      if (Date.now() > deadline) {
        rejectPromise(new Error(`等待 ${HEALTH_URL} 就绪超时（${READY_TIMEOUT_MS}ms）`))
        return
      }
      setTimeout(tick, POLL_INTERVAL_MS)
    }
    void tick()
  })
}

const workDir = mkdtempSync(join(tmpdir(), 'gateway-e2e-'))
const configPath = join(workDir, 'config.json')
writeFileSync(configPath, JSON.stringify(EMPTY_CONFIG), 'utf8')

const binary = resolveBinary()
console.log(`[e2e-gateway] binary : ${binary}`)
console.log(`[e2e-gateway] workdir: ${workDir}`)
console.log(`[e2e-gateway] config : ${configPath}`)
console.log(`[e2e-gateway] listen : ${GATEWAY_ADDR}`)

// P2-3：Linux 上各路径的默认值是 /etc/gateway、/var/lib/gateway、/var/log/gateway，
// 只设 GATEWAY_CONFIG_PATH 会让版本库/队列库/审计日志写到系统目录（CI 无权限或
// 多次运行互相污染）。这里把全部运行数据显式指到本次临时目录，退出即整体删除。
const isolatedEnv = {
  GATEWAY_CONFIG_PATH: configPath,
  GATEWAY_DB_PATH: join(workDir, 'data_queue.db'),
  GATEWAY_VERSION_DB: join(workDir, 'config_versions.db'),
  GATEWAY_OUTBOX_DB: join(workDir, 'mqtt_outbox.db'),
  GATEWAY_AUDIT_LOG: join(workDir, 'audit.log'),
  GATEWAY_DIAG_LOG: join(workDir, 'gateway.log'),
  GATEWAY_LOG_DIR: join(workDir, 'logs'),
  GATEWAY_OTA_STAGING: join(workDir, 'ota'),
  GATEWAY_TEMPLATE_BUILTIN: join(workDir, 'templates', 'builtin'),
  GATEWAY_TEMPLATE_CUSTOM: join(workDir, 'templates', 'custom'),
  GATEWAY_LUA_SCRIPT: join(workDir, 'process.lua'),
  GATEWAY_CERT_DIR: join(workDir, 'certs'),
  GATEWAY_LICENSE_CLOCK_DB: join(workDir, 'license_clock.db'),
}

const child = spawn(binary, {
  cwd: workDir,
  env: { ...process.env, ...isolatedEnv },
  stdio: ['ignore', 'pipe', 'pipe'],
})

child.stdout.on('data', (chunk) => process.stdout.write(`[gateway] ${chunk}`))
child.stderr.on('data', (chunk) => process.stderr.write(`[gateway] ${chunk}`))

let exited = false
function cleanup(signal) {
  if (exited) return
  exited = true
  console.log(`[e2e-gateway] 收到 ${signal ?? 'exit'}，清理子进程与临时目录`)
  // Windows 下 tree-kill 最稳：先尝试 taskkill /T，失败再用 child.kill
  if (process.platform === 'win32') {
    spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore' })
  } else {
    try { child.kill('SIGTERM') } catch { /* 已退出 */ }
  }
  try { rmSync(workDir, { recursive: true, force: true }) } catch { /* 清理尽力而为 */ }
}

process.on('SIGINT', () => { cleanup('SIGINT'); process.exit(0) })
process.on('SIGTERM', () => { cleanup('SIGTERM'); process.exit(0) })

child.on('exit', (code, signal) => {
  if (exited) return
  exited = true
  try { rmSync(workDir, { recursive: true, force: true }) } catch { /* ignore */ }
  console.error(`[e2e-gateway] gatewayd 提前退出 code=${code} signal=${signal}`)
  process.exit(code ?? 1)
})

try {
  await waitForHealthy()
  console.log('[e2e-gateway] 就绪：/health/live 返回 200，夹具保持运行中')
} catch (err) {
  console.error(`[e2e-gateway] ${err.message}`)
  cleanup('health-timeout')
  process.exit(1)
}
