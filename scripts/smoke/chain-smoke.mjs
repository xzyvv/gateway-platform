/**
 * 离线全链路采集冒烟：真实 Modbus TCP 从站 → 真实 gatewayd.exe → 真实轮询 → 入队 → /api/history 读回。
 *
 * 为什么需要它
 * ------------
 * 仓库里已有的自动化各缺一块：
 *   - scripts/e2e_smoke.ps1：真进程真配置，但 buses/points 为空，总线从未打开；
 *   - gateway-api/tests/e2e_journey.rs：用 FakeBus 替换交换层，不走生产传输层；
 *   - south/protocol 各协议的 tests.rs：只测帧编解码，不测「配置 → 采集 → 入队 → 查询」。
 *
 * 本脚本补上这块：Node 起一个真实 Modbus TCP 从站（scripts/smoke/modbus-slave.mjs），
 * 复制 examples/full-config.json 并把 tcp_addr 指向该从站，以临时目录为 cwd 拉起
 * gatewayd.exe，等 /health/live 就绪后轮询 /api/history，断言读回的采集值等于从站寄存器值。
 *
 * 反向验证（默认开启）
 * --------------------
 * 同样的流程再跑一遍，但故意把 address_num 指到另一个寄存器：此时**必须**读到那个
 * 不同的值，且绝不能出现原值。若这一遍也能读到原值，说明正向断言根本没验证寻址，
 * 冒烟本身就是假绿灯——因此反向验证失败同样算整体失败。
 *
 * 用法：
 *   node scripts/smoke/chain-smoke.mjs                # 正向 + 反向
 *   node scripts/smoke/chain-smoke.mjs --skip-negative  # 只跑正向
 *   node scripts/smoke/chain-smoke.mjs --keep-temp      # 失败排查时保留临时目录
 *
 * 环境变量：
 *   GATEWAYD_BIN  指定 gatewayd 可执行文件路径（默认 target/debug/gatewayd.exe）
 *
 * 退出码：0 = 全部通过；1 = 任一断言失败或进程起不来。
 */

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { startSlave } from './modbus-slave.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, '..', '..');
const CONFIG_TEMPLATE = path.join(REPO_ROOT, 'examples', 'full-config.json');

/** 从站 0 号保持寄存器的值：正向断言读它 */
const EXPECTED_VALUE = 4242;
/** 从站 1 号保持寄存器的值：反向验证读它 */
const SHIFTED_VALUE = 111;
/** 点位 ID（与 examples/full-config.json 保持一致） */
const POINT_ID = 'smoke.reg0';

const STARTUP_TIMEOUT_MS = 60_000;
const COLLECT_TIMEOUT_MS = 30_000;

/**
 * 子进程必须完全隔离于宿主环境的 GATEWAY_* 路径覆盖，
 * 否则队列库 / 日志 / 证书会写到仓库根目录（清单见 scripts/e2e_smoke.ps1）。
 */
const PATH_ENV_VARS = [
  'GATEWAY_CONFIG_PATH',
  'GATEWAY_DB_PATH',
  'GATEWAY_OUTBOX_DB',
  'GATEWAY_VERSION_DB',
  'GATEWAY_LICENSE_CLOCK_DB',
  'GATEWAY_WATCHDOG_STATE',
  'GATEWAY_DIAG_LOG',
  'GATEWAY_LOG_DIR',
  'GATEWAY_AUDIT_LOG',
  'GATEWAY_CERT_DIR',
  'GATEWAY_OTA_STAGING',
  'GATEWAY_TEMPLATE_BUILTIN',
  'GATEWAY_TEMPLATE_CUSTOM',
];

function log(msg) {
  console.log(`[chain-smoke] ${msg}`);
}

function fail(msg) {
  throw new Error(msg);
}

/** 查找 gatewayd 可执行文件，顺序：GATEWAYD_BIN → target/debug → gatewayd/target/debug */
function findGatewayd() {
  const candidates = [];
  if (process.env.GATEWAYD_BIN) candidates.push(process.env.GATEWAYD_BIN);
  candidates.push(path.join(REPO_ROOT, 'target', 'debug', 'gatewayd.exe'));
  candidates.push(path.join(REPO_ROOT, 'gatewayd', 'target', 'debug', 'gatewayd.exe'));
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }
  fail(`找不到 gatewayd 可执行文件，候选路径：\n  ${candidates.join('\n  ')}\n请先 cargo build -p gatewayd，或设置 GATEWAYD_BIN`);
}

/** 拿一个当前空闲的本机端口：先 listen(0) 读回端口再关掉，避免与现场/其他测试抢端口 */
function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

/** 极简 JSON GET：不依赖全局 fetch，兼容较老的 Node */
function getJson(url, timeoutMs = 5_000) {
  return new Promise((resolve, reject) => {
    const req = http.get(url, { timeout: timeoutMs }, (res) => {
      let body = '';
      res.setEncoding('utf8');
      res.on('data', (chunk) => {
        body += chunk;
      });
      res.on('end', () => {
        try {
          resolve({ status: res.statusCode, body: JSON.parse(body) });
        } catch {
          resolve({ status: res.statusCode, body: null });
        }
      });
    });
    req.on('timeout', () => req.destroy(new Error(`GET ${url} 超时`)));
    req.on('error', reject);
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function approx(actual, expected) {
  return typeof actual === 'number' && Math.abs(actual - expected) < 1e-6;
}

/** 历史接口被信封中间件包成 {code,message,data}，此处两种形态都兼容 */
function extractItems(payload) {
  if (!payload) return [];
  const inner = payload.data ?? payload;
  return Array.isArray(inner.items) ? inner.items : [];
}

/** 等 /health/live 返回 200；进程提前退出则立刻失败 */
async function waitForLive(baseUrl, proc, errLog) {
  const deadline = Date.now() + STARTUP_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) {
      fail(`gatewayd 启动后立即退出（ExitCode=${proc.exitCode}）\n--- 主进程日志 ---\n${tail(errLog)}`);
    }
    try {
      const { status } = await getJson(`${baseUrl}/health/live`, 2_000);
      if (status === 200) return;
    } catch {
      /* 尚未就绪，继续轮询 */
    }
    await sleep(500);
  }
  fail(`${STARTUP_TIMEOUT_MS / 1000}s 内 /health/live 未就绪\n--- 主进程日志 ---\n${tail(errLog)}`);
}

/** 轮询 /api/history 直到断言成立，返回样本；超时返回 null */
async function waitForSamples(baseUrl, predicate, errLog) {
  const deadline = Date.now() + COLLECT_TIMEOUT_MS;
  let lastItems = [];
  while (Date.now() < deadline) {
    try {
      const { status, body } = await getJson(
        `${baseUrl}/api/history?point_id=${encodeURIComponent(POINT_ID)}&limit=50`,
        5_000,
      );
      if (status === 200) {
        lastItems = extractItems(body);
        if (predicate(lastItems)) return lastItems;
      }
    } catch {
      /* 采集尚未产生数据，继续轮询 */
    }
    await sleep(500);
  }
  if (lastItems.length === 0) {
    fail(`${COLLECT_TIMEOUT_MS / 1000}s 内 /api/history 未返回任何样本（采集链路未打通）\n--- 主进程日志 ---\n${tail(errLog)}`);
  }
  return null;
}

function tail(file, lines = 30) {
  try {
    const content = fs.readFileSync(file, 'utf8').trim();
    if (!content) return '(空)';
    return content.split(/\r?\n/).slice(-lines).join('\n');
  } catch {
    return '(无日志)';
  }
}

/** 停止子进程：先温和 kill，5s 不退则 taskkill /F */
async function stopProc(proc) {
  if (!proc || proc.exitCode !== null) return;
  const exited = new Promise((resolve) => proc.once('exit', resolve));
  proc.kill();
  const timeout = new Promise((resolve) => setTimeout(resolve, 5_000));
  const raced = await Promise.race([exited, timeout]);
  if (raced === undefined && proc.exitCode === null) {
    if (process.platform === 'win32') {
      spawn('taskkill', ['/PID', String(proc.pid), '/T', '/F'], { stdio: 'ignore' });
    } else {
      proc.kill('SIGKILL');
    }
    await Promise.race([exited, sleep(3_000)]);
  }
}

/**
 * 跑一遍完整链路。
 *
 * @param {object} opts
 * @param {string} opts.label        本轮标签（日志用）
 * @param {string} opts.binary       gatewayd 路径
 * @param {{address:string, port:number}} opts.slave 从站地址
 * @param {(cfg:object)=>void} opts.mutate 就地修改配置
 * @param {(items:any[])=>boolean} opts.predicate 样本满足条件即通过
 * @param {string} opts.expectText  通过时的日志描述
 */
async function runPass({ label, binary, slave, mutate, predicate, expectText }) {
  log(`${label}：准备临时工作目录`);
  const workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'gateway-chain-smoke-'));
  const configPath = path.join(workDir, 'config.json');
  const outLog = path.join(workDir, 'gatewayd.out.log');
  const errLog = path.join(workDir, 'gatewayd.err.log');

  const cfg = JSON.parse(fs.readFileSync(CONFIG_TEMPLATE, 'utf8'));
  // 从站地址是动态端口，必须逐轮 patch，配置样例里的 1502 只是占位
  cfg.buses[0].tcp_addr = `${slave.address}`;
  // Web 必须收敛到 127.0.0.1，否则 security_violations() 会直接拒绝启动
  const webPort = await freePort();
  cfg.web.listen_addr = `127.0.0.1:${webPort}`;
  mutate(cfg);

  fs.writeFileSync(configPath, JSON.stringify(cfg, null, 2), 'utf8');

  const childEnv = { ...process.env };
  for (const name of PATH_ENV_VARS) delete childEnv[name];
  // 配置路径白名单要求配置文件位于子进程 cwd 之下，故 cwd 必须是临时目录
  childEnv.GATEWAY_CONFIG_PATH = configPath;

  const outFd = fs.openSync(outLog, 'a');
  const errFd = fs.openSync(errLog, 'a');

  log(`${label}：启动 gatewayd（Web 127.0.0.1:${webPort}，从站 ${slave.address}）`);
  const proc = spawn(binary, [], {
    cwd: workDir,
    env: childEnv,
    stdio: ['ignore', outFd, errFd],
    windowsHide: true,
  });

  let keep = false;
  try {
    const baseUrl = `http://127.0.0.1:${webPort}`;
    await waitForLive(baseUrl, proc, errLog);
    log(`${label}：/health/live 已就绪，等待采集入队`);

    const items = await waitForSamples(baseUrl, predicate, errLog);
    if (items === null) {
      fail(`${label}：${COLLECT_TIMEOUT_MS / 1000}s 内未满足断言「${expectText}」\n--- 主进程日志 ---\n${tail(errLog)}`);
    }
    log(`${label}：PASS，共 ${items.length} 条样本`);
    return { ok: true, items };
  } catch (err) {
    keep = true;
    log(`${label}：FAIL ${err.message}`);
    return { ok: false, error: err.message };
  } finally {
    await stopProc(proc);
    fs.closeSync(outFd);
    fs.closeSync(errFd);
    if (keep || process.argv.includes('--keep-temp')) {
      log(`${label}：临时目录保留 ${workDir}`);
    } else {
      fs.rmSync(workDir, { recursive: true, force: true });
    }
  }
}

async function main() {
  const skipNegative = process.argv.includes('--skip-negative');

  if (!fs.existsSync(CONFIG_TEMPLATE)) {
    fail(`缺少配置样例 ${CONFIG_TEMPLATE}`);
  }
  const binary = findGatewayd();
  log(`gatewayd: ${binary}`);
  log(`配置样例: ${CONFIG_TEMPLATE}`);

  // 从站表：[0]=4242（正向目标），[1]=111（反向目标）
  const slave = await startSlave({ holding: [EXPECTED_VALUE, SHIFTED_VALUE] });
  log(`Modbus 从站已启动：${slave.address}（0 号=${EXPECTED_VALUE}，1 号=${SHIFTED_VALUE}）`);

  const results = [];
  try {
    // ---- 正向：address_num=0 必须读到 4242 ----
    const before = slave.stats.requests;
    results.push(
      await runPass({
        label: '正向',
        binary,
        slave,
        mutate: () => {
          /* examples/full-config.json 原样：reg0 → address_num 0 */
        },
        predicate: (items) => items.some((it) => approx(it.value, EXPECTED_VALUE)),
        expectText: `smoke.reg0.value == ${EXPECTED_VALUE}`,
      }),
    );

    const requests = slave.stats.requests - before;
    if (requests <= 0) {
      results.push({ ok: false, error: '从站未收到任何请求：网关的采集没有走真实 TCP 轮询（断言无效）' });
    } else {
      log(`正向：从站收到 ${requests} 次真实请求（FC 分布：${JSON.stringify(slave.stats.byFunction)}）`);
    }

    // ---- 反向：把 address_num 挪到 1 号寄存器，必须读到 111 且不得再出现 4242 ----
    if (!skipNegative) {
      results.push(
        await runPass({
          label: '反向',
          binary,
          slave,
          mutate: (cfg) => {
            // 只留一个点位，把寻址挪到 1 号寄存器；slave_id 保持不变
            cfg.points = [cfg.points[0]];
            cfg.points[0].address_num = 1;
          },
          predicate: (items) =>
            items.length > 0 &&
            items.every((it) => !approx(it.value, EXPECTED_VALUE)) &&
            items.some((it) => approx(it.value, SHIFTED_VALUE)),
          expectText: `smoke.reg0.address_num=1 时读到 ${SHIFTED_VALUE} 且不出现 ${EXPECTED_VALUE}`,
        }),
      );
    } else {
      log('反向验证已跳过（--skip-negative）');
    }
  } finally {
    await slave.close();
  }

  const failed = results.filter((r) => !r.ok);
  if (failed.length > 0) {
    console.error(`[chain-smoke] FAIL（${failed.length}/${results.length} 轮失败）`);
    for (const r of failed) console.error(`  - ${r.error}`);
    process.exit(1);
  }
  console.log(`[chain-smoke] PASS（${results.length}/${results.length} 轮通过）`);
  process.exit(0);
}

main().catch((err) => {
  console.error(`[chain-smoke] FAIL ${err.message}`);
  process.exit(1);
});
