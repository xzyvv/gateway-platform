// 临时核验脚本：对 17 号报告的每条具体断言逐条复核
import { readFileSync, existsSync, readdirSync } from 'node:fs'
import { join, resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const web = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const src = join(web, 'src')
const repo = resolve(web, '..')

let pass = 0, fail = 0
function check(id, desc, ok, detail = '') {
  const mark = ok ? 'PASS' : '**FAIL**'
  if (ok) pass++; else fail++
  console.log(`[${mark}] ${id} ${desc}${detail ? '  -- ' + detail : ''}`)
}
function read(p) {
  const full = p.startsWith('src/') ? join(src, p.slice(4)) : join(repo, p)
  return existsSync(full) ? readFileSync(full, 'utf8') : null
}
function lineOf(text, substr) {
  if (!text) return -1
  const lines = text.split(/\r?\n/)
  for (let i = 0; i < lines.length; i++) if (lines[i].includes(substr)) return i + 1
  return -1
}
// 去掉行注释与块注释，只留可执行代码。
// 断言必须区分「注释里提到某个旧路径」与「真的 import/调用了它」——
// 修 DD-66 时源码注释本身就会写明"原先是 xxx"，正则不区分就会误判。
function stripComments(text) {
  if (!text) return ''
  return text
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:])\/\/.*$/gm, '$1')
}
// 递归列出目录下所有源文件（跳过生成物与超大目录），供全仓符号残留扫描使用
const SKIP_DIR = new Set(['generated', 'node_modules', 'dist', 'coverage'])
const SKIP_FILE = new Set(['ep-icons-compact.ts'])
function listFiles(dir) {
  const out = []
  let entries = []
  try { entries = readdirSync(dir, { withFileTypes: true }) } catch { return out }
  for (const e of entries) {
    const full = join(dir, e.name)
    if (e.isDirectory()) {
      if (SKIP_DIR.has(e.name)) continue
      out.push(...listFiles(full))
    } else if (e.isFile()) {
      if (SKIP_FILE.has(e.name)) continue
      if (!/\.(ts|tsx|vue|js|mjs)$/.test(e.name)) continue
      out.push(full)
    }
  }
  return out
}

// 文档内容统一在此读取，供各断言块引用（DD-66 断言块在 D 类之前，故提前声明）
const fm = read('docs/architecture/frontend.md')
const fn = read('docs/architecture/frontend-naming.md')
const fst = read('docs/architecture/frontend-state.md')
const creadme = read('gateway-web/src/composables/README.md')
const treadme = read('gateway-web/src/test/README.md')
const greadme = read('gateway-web/README.md')

console.log('=== A-1 静态环根因：responseInterceptor 静态引 3 个 store ===')
const ri = read('src/api/http/responseInterceptor.ts')
check('A-1.1', '【③ 已修】responseInterceptor 不再静态引 stores/notifications（改走 uiEvents 端口）',
  lineOf(ri, "stores/notifications") === -1 && lineOf(ri, 'emitHttpNotify') > 0)
check('A-1.2', '【③ 已修】responseInterceptor 不再静态引 stores/requestLog（改走 emitRequestLog）',
  lineOf(ri, "stores/requestLog") === -1 && lineOf(ri, 'emitRequestLog') > 0)
check('A-1.3', "【DD-65 已修】responseInterceptor 不再静态引 stores/auth",
  !/from '\.\.\/\.\.\/stores\/auth'/.test(ri || ''))
check('A-1.4', '同文件已用 void import 断环（跳登录）',
  lineOf(ri, "import('../../router')") > 0, `line ${lineOf(ri, "import('../../router')")}`)

console.log('\n=== DD-64 lint 红灯已修 ===')
const sse = read('src/api/protocolTools/sse.ts')
check('DD-64.1', 'sse.ts 不再有裸 console.*', !/console\.(warn|log|error|info)\(/.test(sse || ''))
check('DD-64.2', 'sse.ts 改用 @/utils/logger 的 logWarn', /logWarn\('protocolTools'/.test(sse || ''), `line ${lineOf(sse, "logWarn('protocolTools'")}`)

console.log('\n=== DD-65 补丁桥已落地 ===')
const bridge = read('src/api/http/authBridge.ts')
const main = read('src/app/main.ts')
check('DD-65.1', 'api/http/authBridge.ts 存在', bridge !== null)
check('DD-65.2', 'authBridge 导出 setAuthLogoutPatcher / applyAuthLogoutPatch',
  /export function setAuthLogoutPatcher/.test(bridge || '') && /export function applyAuthLogoutPatch/.test(bridge || ''))
check('DD-65.3', 'responseInterceptor 调用 applyAuthLogoutPatch()', /applyAuthLogoutPatch\(\)/.test(ri || ''), `line ${lineOf(ri, 'applyAuthLogoutPatch()')}`)
check('DD-65.4', 'app/main.ts 注册补丁函数', /setAuthLogoutPatcher\(/.test(main || ''), `line ${lineOf(main, 'setAuthLogoutPatcher(')}`)
check('DD-65.5', 'authBridge 有回归用例', read('src/api/http/authBridge.spec.ts') !== null)

console.log('\n=== A-2 / DD-66 auth ⟷ useSessions 2 元环（已修：上移为 store + 断边）===')
const auth = read('src/stores/auth.ts')
const sess = read('src/stores/sessions.ts')
const sessSpec = read('src/stores/sessions.spec.ts')
const loginVue = read('src/views/auth/Login.vue')
const useSystem = read('src/composables/system/useSystem.ts')
check('A-2.1', "【DD-66 已修】stores/auth.ts 不再静态引会话模块（auth → sessions 边已断）",
  !/composables\/system\/useSessions/.test(stripComments(auth || '')) && !/from '\.\/sessions'/.test(stripComments(auth || '')),
  `残留 import ${lineOf(stripComments(auth || ''), 'useSessions')}`)
check('A-2.2', "【DD-66 已修】会话能力上移为 stores/sessions.ts，且静态引 stores/auth（单向）",
  /export const useSessionsStore = defineStore\('sessions'/.test(stripComments(sess || '')) &&
  /from '\.\/auth'/.test(stripComments(sess || '')),
  `defineStore line ${lineOf(sess, "export const useSessionsStore")}`)
check('A-2.3', '【DD-66 已修】auth.ts 内不再调 useSessions().markLogin()',
  !/useSessions\(\)\.markLogin\(\)/.test(stripComments(auth || '')))
check('A-2.4', "sessions store 保留 useAuthStore().$patch 的真登出语义（Bug 180）",
  lineOf(sess, 'useAuthStore().$patch') > 0, `line ${lineOf(sess, 'useAuthStore().$patch')}`)
check('DD-66.1', 'stores/sessions.ts 存在且导出 LoginSession 类型',
  sess !== null && /export interface LoginSession/.test(sess || ''))
check('DD-66.2', 'stores/sessions.spec.ts 存在（用例已随实现迁移）', sessSpec !== null)
check('DD-66.3', '旧文件 composables/system/useSessions.ts 已删除',
  read('src/composables/system/useSessions.ts') === null)
check('DD-66.4', '旧 spec composables/system/useSessions.spec.ts 已删除',
  read('src/composables/system/useSessions.spec.ts') === null)
check('DD-66.5', 'Login.vue 登录成功后显式登记会话 useSessionsStore().markLogin()',
  lineOf(loginVue, 'useSessionsStore().markLogin()') > 0, `line ${lineOf(loginVue, 'useSessionsStore().markLogin()')}`)
check('DD-66.6', 'useSystem.ts 改用 useSessionsStore',
  /import \{ useSessionsStore \} from '@\/stores\/sessions'/.test(useSystem || ''),
  `line ${lineOf(useSystem, "useSessionsStore } from '@/stores/sessions'")}`)
check('DD-66.7', "frontend-state.md 的 store 清单含 sessions，且不再把 useSessions 登记为 @singleton",
  /\| `sessions` \|/.test(fst || '') && !/\| `useSessions` \|/.test(fst || ''))
check('DD-66.8', 'composables/README.md 不再把 useSessions 列为 composable',
  !/\| useSessions \|/.test(creadme || ''))
check('DD-66.9', '全仓已无指向 composables/system/useSessions 的 import',
  ![...listFiles(src)].some((f) => /composables\/system\/useSessions/.test(stripComments(readFileSync(f, 'utf8')))))

console.log('\n=== A-3 唯一真层级倒挂：composable 引用 view 组件 ===')
const ud = read('src/composables/devices/useDevices/index.ts')
check('A-3.1', '【DD-67 已修】useDevices/index.ts 不再 import view 组件，对话框改由 DeviceList.vue 挂载',
  lineOf(ud, "@/views/devices/dialogs/DeviceDiscoverDialog.vue") === -1 &&
  lineOf(read('src/views/devices/DeviceList.vue'), "from './dialogs/DeviceDiscoverDialog.vue'") > 0)

console.log('\n=== A-4 utils 反向依赖 + api⟷utils 双向 ===')
const ccNew = read('src/composables/core/commonColumns.ts')
check('A-4.1', '【DD-86 已修】commonColumns 迁入 composables，utils 不再 → composables',
  read('src/utils/commonColumns.ts') === null && lineOf(ccNew, "@/composables/devices/protocolMeta") > 0)
check('A-4.2', '【DD-86 已修】ColumnDef 下沉 types/dataTable.ts，composables 不再 → components',
  lineOf(ccNew, "@/types/dataTable") > 0 && read('src/types/dataTable.ts') !== null)
const pool = read('src/utils/websocket/pool.ts')
check('A-4.3', '【DD-86 已修】pool 经 publisher 端口上报，不再直接引 stores/websocket',
  lineOf(pool, 'stores/websocket') === -1)
check('A-4.4', '【DD-86 已修】state.ts 已删除，发布逻辑回到 stores 层',
  read('src/utils/websocket/state.ts') === null)
const gwStorage = read('src/utils/gatewayStorage.ts')
check('A-4.5', '【DD-86 已修】gatewayStorage 经 gatewayContext 读网关名，不再反向依赖 api',
  lineOf(gwStorage, '@/api') === -1)

console.log('\n=== S-2 三个 0 字节垃圾文件（DD-82 已修）===')
for (const f of ['c', 'r', 'v']) {
  const p = join(src, f)
  const ex = existsSync(p)
  check(`S-2.${f}`, `【DD-82 已修】src/${f} 0 字节垃圾文件已删除`, !ex, ex ? '仍存在' : '已删除')
}

console.log('\n=== S-4 utils/deviceComms.ts 模块级 reactive ===')
const dc = read('src/utils/deviceComms.ts')
check('S-4.1', '【DD-84 已修】deviceComms.ts 的模块级 reactive 已显式豁免（不再是无标注盲区）',
  /eslint-disable-next-line no-restricted-syntax[\s\S]{0,300}const STATS = reactive\(new Map/.test(dc || ''),
  `line ${lineOf(dc, 'reactive(new Map')}`)
const eslint = read('gateway-web/eslint.config.js')
check('S-4.2', '【DD-84 已修】ESLint #17 规则覆盖 src/composables/**/*.ts 与 src/utils/**/*.ts',
  /files: \['src\/composables\/\*\*\/\*\.ts', 'src\/utils\/\*\*\/\*\.ts'\]/.test(eslint || ''))

console.log('\n=== E-1 组件状态取色绕开唯一真源 ===')
const hr = read('src/views/ops/health/HealthReport.vue')
check('E-1.1', '【DD-68 已修】HealthReport.vue 不再自建 componentType 映射',
  !/const componentType = \(v: string\)/.test(hr || ''))
const hs = read('src/utils/healthStatus.ts')
check('E-1.2', "healthStatus.ts 登记 error → severity error",
  /error: \{ label: '异常', severity: 'error' \}/.test(hs || ''), `line ${lineOf(hs, "error: { label: '异常'")}`)
check('E-1.3', "【DD-77 已修】healthStatus.ts 已删除死导出 healthStatusTag，组件标签语义由 healthComponentLabel 承接",
  !/healthStatusTag/.test(hs || '') && /export function healthComponentLabel/.test(hs || ''),
  `healthComponentLabel line ${lineOf(hs, 'export function healthComponentLabel')}`)
check('E-1.4', '【DD-68 已修】HealthReport.vue 改用 healthStatusInfo + healthComponentLabel',
  /healthStatusInfo/.test(hr || '') && /healthComponentLabel/.test(hr || ''))

console.log('\n=== E-2 问题分级计数两份 ===')
const hstore = read('src/stores/healthReport.ts')
const hbar = read('src/views/ops/health/HealthConclusionBar.vue')
check('E-2.1', 'stores/healthReport.ts 仍持有 problemCounts computed', /const problemCounts = computed/.test(hstore || ''), `line ${lineOf(hstore, 'const problemCounts = computed')}`)
check('E-2.2', '【DD-69 已修】HealthConclusionBar 不再自带 reduce，改用 countProblemLevels',
  !/c\[p\.level\] \+= 1/.test(hbar || '') && /countProblemLevels/.test(hbar || ''))
check('E-2.3', '【DD-69 已修】store 的 problemCounts 走同一个 countProblemLevels',
  /countProblemLevels/.test(hstore || ''), `line ${lineOf(hstore, 'countProblemLevels')}`)
check('E-2.4', '【DD-69 已修】store 的 problemCounts 被页面真实消费（结论条 counts prop）',
  /:counts="problemCounts"/.test(read('src/views/ops/health/GatewayDiag.vue') || '') &&
  /:counts="healthProblemCounts"/.test(read('src/views/system/overview/SystemOverview.vue') || ''))

console.log('\n=== E-3 健康状态中文名两份 ===')
check('E-3.1', '【DD-70 已修】stores/healthReport.ts 不再自建 STATUS_META 字面量',
  !/const STATUS_META/.test(hstore || '') && /overallStatusMeta/.test(hstore || ''))
const uhs = read('src/composables/dashboard/useHealthSignals.ts')
check('E-3.2', '【DD-70 已修】useHealthSignals.ts 不再自建 STATUS_LABEL 字面量',
  !/const STATUS_LABEL/.test(uhs || '') && /overallStatusMeta/.test(uhs || ''))
check('E-3.3', '【DD-70 已修】两处中文名同源到 utils/healthStatus 的 overallStatusMeta',
  /export function overallStatusMeta/.test(hs || ''))

console.log('\n=== E-4 磁盘水位档位两份 ===')
const dh = read('src/views/ops/health/DiskHealthChart.vue')
check('E-4.1', '【DD-71 已修】HealthReport.vue 不再自建 DISK_LEVEL_LABEL',
  !/const DISK_LEVEL_LABEL/.test(hr || '') && /diskLevelMeta/.test(hr || ''))
check('E-4.2', '【DD-71 已修】DiskHealthChart.vue 不再自建 LEVEL_LABEL',
  !/const LEVEL_LABEL/.test(dh || '') && /diskLevelMeta/.test(dh || ''))
check('E-4.3', '【DD-71 已修】磁盘档位同源到 utils/healthStatus 的 diskLevelMeta',
  /export function diskLevelMeta/.test(hs || ''))

console.log('\n=== E-5 健康问题分级取色 ===')
const hpl = read('src/views/ops/health/HealthProblemList.vue')
const dwp = read('src/views/acquisition/console/components/debugWizardProbes.ts')
check('E-5.1', '【DD-72 已修】HealthProblemList.vue 委托 problemLevelMeta',
  !/level === 'critical'/.test(hpl || '') && /problemLevelMeta/.test(hpl || ''))
check('E-5.2', '【DD-72 已修】debugWizardProbes.ts 委托 problemLevelMeta',
  !/level === 'critical'/.test(dwp || '') && /problemLevelMeta/.test(dwp || ''))
check('E-5.3', '【DD-72 已修】问题分级同源到 utils/healthStatus 的 problemLevelMeta',
  /export function problemLevelMeta/.test(hs || ''))

console.log('\n=== E-6 diagnostics 双 export * ===')
const dindex = read('src/api/diagnostics/index.ts')
check('E-6.1', '【DD-73 已修】diagnostics 双 export* barrel 已删除，调用方显式从 raw/view 导入',
  dindex === null)
const busload = read('src/views/ops/bus/BusLoad.vue')
check('E-6.2', 'BusLoad.vue 靠重命名消歧 listBusLoadsView as listBusLoads',
  /listBusLoadsView as listBusLoads/.test(busload || ''), `line ${lineOf(busload, 'listBusLoadsView as listBusLoads')}`)

console.log('\n=== E-7 两个同名 ConfigIssue ===')
const cv = read('src/composables/config/configValidation.ts')
const ch = read('src/composables/config/configHealth.ts')
check('E-7.1', '【DD-74 已修】configValidation.ts 的原始问题改名为 ConfigValidationIssue',
  /export interface ConfigValidationIssue/.test(cv || '') && !/export interface ConfigIssue\b/.test(cv || ''),
  `line ${lineOf(cv, 'export interface ConfigValidationIssue')}`)
check('E-7.2', '【DD-74 已修】configHealth.ts 的展示问题改名为 ConfigHealthIssue',
  /export interface ConfigHealthIssue/.test(ch || '') && !/export interface ConfigIssue\b/.test(ch || ''),
  `line ${lineOf(ch, 'export interface ConfigHealthIssue')}`)

console.log('\n=== E-8/E-9 术语漂移 ===')
const ptools = read('src/api/protocolTools/types.ts')
const upl = read('src/api/uplink.ts')
check('E-8.1', 'useSessions.ts 定义 LoginSession', /export interface LoginSession/.test(sess || ''), `line ${lineOf(sess, 'export interface LoginSession')}`)
check('E-8.2', 'protocolTools/types.ts 定义 ReplaySession', /export interface ReplaySession/.test(ptools || ''), `line ${lineOf(ptools, 'export interface ReplaySession')}`)
check('E-8.3', 'api/uplink.ts 定义 PassthroughSession', /export interface PassthroughSession/.test(upl || ''), `line ${lineOf(upl, 'export interface PassthroughSession')}`)
check('E-8.4', '【DD-75 已修】命名文档登记会话三词口径，README 无「连接会话」',
  /登录会话.*回放会话.*透传会话/.test(read('docs/architecture/frontend-naming.md') || '') &&
  !/连接会话/.test(read('src/composables/README.md') || ''))
check('E-9.1', '【DD-75 已修】设备评分与网关健康的 critical 统一为「严重」',
  /critical: '严重'/.test(read('src/utils/healthVerdict.ts') || '') &&
  /critical: \{ label: '严重'/.test(read('src/utils/healthStatus.ts') || ''))

console.log('\n=== E-14 源码 UTF-8 残字节（DD-80 已修）===')
const conc = read('src/utils/concurrency.ts')
check('E-14.1', "【DD-80 已修】concurrency.ts 不再含 U+FFFD 残字节", !(conc || '').includes('\uFFFD'), `残留行 ${lineOf(conc, '\uFFFD')}`)
check('E-14.2', 'concurrency.ts:61 附近有完整句子「已在飞的任务照常跑完」（修复时的对照句仍在）', /已在飞的任务照常跑完/.test(conc || ''))

console.log('\n=== D 类文档债 ===')
check('D-1', '【DOC-F5 已修】frontend.md 指向 api/env.ts 而非不存在的 utils/tauri.ts',
  !/utils\/tauri\.ts/.test(fm || '') && /api\/env\.ts::isTauri\(\)/.test(fm || ''))
check('D-1b', 'src/utils/tauri.ts 确实不存在', !existsSync(join(src, 'utils', 'tauri.ts')))
check('D-2', '【DOC-F5 已修】frontend.md store 清单不再含 config，改为真实 8 个',
  !/auth\/config\/gateway/.test(fm || '') && /auth\/gateway\/healthReport\/notifications/.test(fm || ''))
check('D-3', '【DOC-F5 已修】frontend.md composables 目录清单含 acquisition',
  /acquisition/.test((fm || '').split('composables/')[1]?.split('mock/')[0] || ''))
check('D-4', '【DOC-F3 已修】frontend.md 不再写「不再为 tab 登记 redirect 别名」',
  !/不再为 tab 登记 redirect 别名/.test(fm || ''))
check('D-4b', '【DOC-F3 已修】frontend-naming.md 同款断言已改', !/不再为每个 tab 登记 redirect 别名/.test(fn || ''))
const routes = read('src/router/routes/index.ts')
const redirects = (routes.match(/redirect:/g) || []).length
check('D-4c', `routes/index.ts 实际 redirect 条数 = 11`, redirects === 11, `实际 ${redirects}`)
check('D-4d', '【DOC-F3 已修】routes/index.ts 注释不再与同文件 11 条 redirect 自相矛盾',
  !/不再为每个 tab 单独登记 redirect 别名/.test(routes || ''))
check('D-5', '【DOC-F3 已修】文档已允许 redirect 别名路由用 *Redirect 命名',
  /QuickSetupRedirect/.test(routes || '') && /BatchMonitorRedirect/.test(routes || '') &&
  /`\*Redirect` 结尾/.test(fn || ''))
check('D-6', '【DOC-F4 已修】frontend-state.md store 清单不再含 config/devices',
  !/\| `config` \|/.test(fst || '') && !/\| `devices` \|/.test(fst || ''))
check('D-7', '【DOC-F4 已修】frontend-state.md 不再把 useRequestLog 列为 composable',
  !/\| `useRequestLog` \|/.test(fst || ''))
check('D-8', '【DOC-F7 已修】composables/README.md 不再写「12 个 .spec.ts」', !/12 个/.test(creadme || ''))
check('D-9', '【DOC-F6 已修】test/README.md 覆盖率阈值改为实测值',
  /lines 63%/.test(treadme || '') && !/lines \*\*30%\*\*/.test(treadme || ''))
check('D-10', '【DOC-F3 已修】frontend-naming.md 不再写「旧地址不做兼容」', !/旧地址不做兼容/.test(fn || ''))
check('D-11', '【DOC-F7 已修】composables/README.md 不再写「9 个子目录」', !/9 个子目录/.test(creadme || ''))
check('D-15', '【DOC-F8 已修】frontend.md：Element Plus 改为按需自动引入，不再写「全局注册」',
  !/全局注册，配中文语言包/.test(fm || '') && /按需自动引入/.test(fm || ''))
check('D-16', '【DOC-F8 已修】frontend.md：Axios 拦截器归属 api/http/，不再写 api/index.ts 封装',
  !/`api\/index\.ts` 封装拦截器/.test(fm || '') && /传输层在 `api\/http\//.test(fm || ''))
check('D-17', '【DOC-F8 已修】frontend.md：E2E 覆盖项已移除暗色/404 两例（转由 vitest 覆盖）',
  !/暗色持久化 \//.test(fm || '') && /已由 vitest/.test(fm || ''))
check('D-18', '【DOC-F9 已修】frontend-naming.md §7 已删 PointTemplates / DataEncryption',
  !/`PointTemplates`/.test(fn || '') && !/`DataEncryption`/.test(fn || ''))
check('D-19', '【DOC-F9 已修】frontend-naming.md：CommTuningHub 中文名为「采集调优」',
  /`CommTuningHub` \| 采集调优/.test(fn || ''))
check('D-20', '【DOC-F9 已修】frontend-naming.md 不再写不存在的 .hub-body-parent',
  !/\.hub-body-parent/.test(fn || '') && /\.hub-body-title/.test(fn || ''))
check('D-21', '【DOC-F10 已修】README.md 写「12 个一级目录」且不再写「13 个」',
  !/13 个一级目录/.test(greadme || '') && /12 个一级目录/.test(greadme || ''))
check('D-24', '【DOC-F10 已修】README.md 优化方向改为「已完成 + 剩余」',
  /已完成（截至 2026-11-28）/.test(greadme || '') && /10 个 Hub \/ 72 个页签/.test(greadme || ''))

console.log('\n=== DD-77 死导出已清理 ===')
check('DD-77.1', 'hasBlockingIssues 已删除', !/hasBlockingIssues/.test(read('src/utils/configValidation.ts') || ''))
check('DD-77.2', 'healthStatusTag 已删除', !/healthStatusTag/.test(read('src/utils/healthStatus.ts') || ''))
check('DD-77.3', 'healthRowsFrom 已删除', !/\bhealthRowsFrom\b/.test(read('src/utils/healthStatus.ts') || ''))
check('DD-77.4', 'invalidateHealthReport 已删除', !/invalidateHealthReport/.test(read('src/composables/system/useHealthReport.ts') || ''))
check('DD-77.5', 'getDeviceStat 已删除', !/getDeviceStat\b/.test(read('src/utils/deviceComms.ts') || ''))
check('DD-77.6', '全仓再无这些符号的引用',
  ![...['hasBlockingIssues', 'healthStatusTag', 'healthRowsFrom', 'invalidateHealthReport', 'getDeviceStat']
    ].some((s) => {
      let hit = false
      for (const f of listFiles(src)) {
        const t = readFileSync(f, 'utf8')
        if (new RegExp(`\\b${s}\\b`).test(t)) hit = true
      }
      return hit
    }))
check('DD-77.7', 'clearDeviceStats 保留（测试夹具，有独立行为用例）',
  /clearDeviceStats/.test(read('src/utils/deviceComms.ts') || ''))
check('DD-77.8', 'diagHint 保留（生产被 withDiagHint 调用，非死导出）',
  /diagHint\(message\)/.test(read('src/utils/diagHints.ts') || ''))

console.log('\n=== DD-80 / DD-82 结构与字符债已清 ===')
check('DD-80.1', 'concurrency.ts 不含 U+FFFD 残字节', !/\uFFFD/.test(conc || ''))
check('DD-80.2', 'concurrency.ts:30 已是完整句子「已在飞的任务不被打断」',
  lineOf(conc, '已在飞的任务不被打断，这与原实现逐字一致') > 0,
  `line ${lineOf(conc, '已在飞的任务不被打断')}`)
check('DD-80.3', '全仓 src 无 U+FFFD 残字节',
  ![...listFiles(src)].some((f) => /\uFFFD/.test(readFileSync(f, 'utf8'))))
check('DD-82.1', 'src/c 0 字节垃圾文件已删除', !existsSync(join(src, 'c')))
check('DD-82.2', 'src/r 0 字节垃圾文件已删除', !existsSync(join(src, 'r')))
check('DD-82.3', 'src/v 0 字节垃圾文件已删除', !existsSync(join(src, 'v')))
const srcDirs = readdirSync(src, { withFileTypes: true }).filter((e) => e.isDirectory()).length
check('DD-82.4', 'src 一级目录数 = 12（与 README 同一句列举的 12 个一致）',
  srcDirs === 12, `实际 ${srcDirs}`)

console.log(`\n=== 汇总: PASS ${pass} / FAIL ${fail} ===`)
