/**
 * 路由一致性元测试
 *
 * 路由已收敛为 `/<业务域>[/<视图能力>]`，不再保留 redirect 兼容别名；站内跳转统一
 * 由侧栏搜索索引（navIndex.ts）与左侧菜单（groups/*.ts）声明 `'/<域>?tab=<name>'`。
 * 历史教训：tab 改名或挪位时这些引用会静默断链（如 /frame-parser 曾指向从未存在的
 * tab，用户点击后回退到别的页面）。
 *
 * 本测试守护五类引用不悬空：
 *   1. 所有 redirect 的目标路径必须已注册（保留兜底：后续若重新引入 redirect）
 *   2. 所有 ?tab= 必须存在于目标 Hub 组件声明的 tabs 中
 *   3. 搜索索引 / 菜单组的所有跳转目标同样校验（1+2）
 *   4. 每个 Hub 页签必须有菜单/搜索索引入口（精确 ?tab= 索引项，或裸路径索引项 + 首个页签）
 *   5. 每个菜单标题必须能被侧栏搜索搜到（title/alias/subtitle，
 *      菜单名与索引项名不一致时在 NavIndexItem.alias 登记菜单名）
 *
 * Hub tab 名的解析方式：读取 Hub 组件源码中 `name: 'xxx'` 声明（HubShell 的
 * HubTab.name 字段），静态正则提取，新增 tab 无需维护本测试。
 */
import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { join, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { routes as routeRecords } from './index'

const here = dirname(fileURLToPath(import.meta.url))
const srcRoot = resolve(here, '../..')

interface RouteLike {
  path: string
  redirect?: string
  component?: unknown
  children?: RouteLike[]
}

function flatten(rs: RouteLike[]): RouteLike[] {
  const out: RouteLike[] = []
  for (const r of rs) {
    out.push(r)
    if (Array.isArray(r.children)) out.push(...flatten(r.children))
  }
  return out
}

// 收集全部路由记录（静态导入 routes/index.ts）。
// 该模块为纯数据模块（仅导出路由表），无副作用，静态导入以便测试加载时即读取其内容。
function loadRoutes(): RouteLike[] {
  return flatten(routeRecords as unknown as RouteLike[])
}

// 从路由文件源码中提取 path -> 组件文件 映射（按对象块切分，避免跨对象误配）
function buildComponentMap(): Record<string, string> {
  const map: Record<string, string> = {}
  for (const f of ['index.ts']) {
    const text = readFileSync(join(here, f), 'utf-8')
    for (const chunk of text.split(/\n {2}\},/)) {
      const p = chunk.match(/path: '([^']+)'/)
      const c = chunk.match(/import\('(@\/[^']+)'\)/)
      if (p?.[1] && c?.[1]) map[p[1]] = c[1]
    }
  }
  return map
}

// Hub 组件源码中声明的 tab 名集合（HubTab.name 字段）
function hubTabNames(componentRef: string): Set<string> {
  const file = resolve(srcRoot, componentRef.replace(/^@\//, ''))
  const text = readFileSync(file, 'utf-8')
  const names = new Set<string>()
  for (const m of text.matchAll(/name: '([\w-]+)'/g)) if (m[1]) names.add(m[1])
  return names
}

// Hub 组件判定：路由组件源码引用 HubShell 即为 Hub 容器。
// 误判无害（无 name+label 声明时页签集为空，循环空转）；漏判不存在（19 个 Hub 均引用）。
function isHubComponent(componentRef: string): boolean {
  return readFileSync(resolve(srcRoot, componentRef.replace(/^@\//, '')), 'utf-8').includes('HubShell')
}

// Hub 组件的页签声明（有序）。tab 对象字面量均为 `{ name: 'xxx', label: ... }` 相邻结构
// （已核对 19 个 Hub 文件中全部 96 处 `name:` 均符合此形态，比裸 name: 正则更抗误配）。
// first 用于「裸路径命令 = 打开首个页签」的默认 tab 规则判定。
function hubTabs(componentRef: string): { names: string[]; first?: string } {
  const text = readFileSync(resolve(srcRoot, componentRef.replace(/^@\//, '')), 'utf-8')
  const names: string[] = []
  for (const m of text.matchAll(/name: '([\w-]+)',\s*label:/g)) if (m[1]) names.push(m[1])
  return { names, first: names[0] }
}

// 解析 '/xxx?tab=yyy' → { base, tab }
function splitTarget(t: string): { base: string; tab?: string } {
  const i = t.indexOf('?')
  if (i < 0) return { base: t }
  const q = new URLSearchParams(t.slice(i + 1))
  return { base: t.slice(0, i), tab: q.get('tab') ?? undefined }
}

const routes = loadRoutes()
const allPaths = new Set(routes.map((r) => r.path))
const realPaths = new Set(routes.filter((r) => !r.redirect).map((r) => r.path))
const compMap = buildComponentMap()

// redirect 记录（来自路由文件）
const redirects = routes.filter((r) => typeof r.redirect === 'string') as {
  path: string
  redirect: string
}[]

// 搜索索引与菜单组的跳转目标（引号包裹的站内路径）
function collectNavTargets(): string[] {
  const files = [
    join(srcRoot, 'views/layout/menu/navIndex.ts'),
    ...readdirSafe(join(srcRoot, 'views/layout/menu/groups')),
  ]
  // 支持多段路径（/<域>/<能力>，如 /devices/topology）
  const re = /'(\/[\w/-]+(?:\?tab=[\w-]+)?)'/g
  const out: string[] = []
  for (const f of files) {
    for (const m of readFileSync(f, 'utf-8').matchAll(re)) if (m[1]) out.push(m[1])
  }
  return out
}

function readdirSafe(dir: string): string[] {
  try {
    return readdirSync(dir)
      .filter((f) => f.endsWith('.ts') && !f.endsWith('.spec.ts'))
      .map((f) => join(dir, f))
  } catch {
    return []
  }
}

function validateTarget(t: string, kind: string): string[] {
  const errs: string[] = []
  const { base, tab } = splitTarget(t)
  if (!allPaths.has(base)) return [`${kind} 指向未注册路径: ${t}`]
  if (tab) {
    if (!realPaths.has(base)) {
      errs.push(`${kind} 的 tab 校验目标不是真实页面路由: ${t}`)
      return errs
    }
    const comp = compMap[base]
    if (!comp) {
      errs.push(`${kind} 目标 ${base} 未找到组件映射，无法校验 tab`)
      return errs
    }
    const names = hubTabNames(comp)
    if (!names.has(tab)) {
      errs.push(`${kind} 的 tab '${tab}' 不存在于 ${comp}（可用: ${[...names].join(', ')}）`)
    }
  }
  return errs
}

/**
 * 反向可达性白名单：这些真实页面路由不经左侧菜单 / 搜索索引暴露，
 * 而是通过 redirect 兼容跳转或编程式导航到达（登记时需注明原因）。
 *
 * 作用：routes/index.ts 里新增了一个真实页面但忘了在
 * views/layout/menu/groups/*.ts 或 navIndex.ts 登记入口时，下方测试会失败，
 * 从而把"新增页面需改 4 处登记点"的漏登记漂移挡在 CI 之外。
 */
const unreachableWhitelist: Record<string, string> = {}

describe('路由一致性元测试', () => {
  it('每个真实页面路由都应被菜单或搜索索引引用（漏登记守护）', () => {
    expect(realPaths.size, '未解析到任何真实页面路由，断言会空转').toBeGreaterThan(0)
    const referenced = new Set(collectNavTargets().map((t) => splitTarget(t).base))
    const unreferenced = [...realPaths].filter(
      (p) => !referenced.has(p) && !(p in unreachableWhitelist),
    )
    expect(
      unreferenced,
      `以下真实页面未登记菜单/搜索索引入口，也未加入 unreachableWhitelist：\n${unreferenced.join('\n')}`,
    ).toEqual([])
  })
  it('所有 redirect 的目标路径必须已注册', () => {
    const errs = redirects
      .map((r) => validateTarget(r.redirect, `redirect ${r.path}`))
      .flat()
    expect(errs, errs.join('\n')).toEqual([])
  })

  it('redirect 的 ?tab= 必须存在于目标 Hub 的 tabs 中', () => {
    const errs = redirects
      .filter((r) => r.redirect.includes('?tab='))
      .map((r) => validateTarget(r.redirect, `redirect ${r.path}`))
      .flat()
    expect(errs, errs.join('\n')).toEqual([])
  })

  it('搜索索引与菜单组的目标路径必须已注册且 tab 存在', () => {
    const errs = collectNavTargets()
      .map((t) => validateTarget(t, `nav ${t}`))
      .flat()
    expect(errs, errs.join('\n')).toEqual([])
  })

  it('每个 Hub 页签都能从菜单/搜索索引到达（tab 覆盖守护）', () => {
    const targets = collectNavTargets()
    const tabTargets = new Set(targets.filter((t) => t.includes('?tab=')))
    const bareBases = new Set(
      targets.filter((t) => !t.includes('?tab=')).map((t) => splitTarget(t).base),
    )
    let tabTotal = 0
    const missing: string[] = []
    for (const [routePath, comp] of Object.entries(compMap)) {
      if (!isHubComponent(comp)) continue
      const { names, first } = hubTabs(comp)
      for (const name of names) {
        tabTotal++
        // 两种可达方式：显式 ?tab= 索引项；或裸路径索引项落在该 Hub 且页签是首个页签
        //（Hub 对无 tab 参数的访问默认渲染首个页签，故首 tab 由裸路径入口覆盖）
        const exact = tabTargets.has(`${routePath}?tab=${name}`)
        const viaFirstTab = bareBases.has(routePath) && name === first
        if (!exact && !viaFirstTab) missing.push(`${routePath}?tab=${name}（${comp}）`)
      }
    }
    expect(tabTotal, '解析到的 Hub 页签过少，name/label 提取正则可能腐坏').toBeGreaterThanOrEqual(60)
    expect(
      missing,
      `以下 Hub 页签在菜单/搜索索引无入口，用户只能靠翻页签发现：\n${missing.join('\n')}`,
    ).toEqual([])
  })

  it('每个菜单标题都能在侧栏搜索命中（alias 词典守护）', () => {
    const cmdText = readFileSync(join(srcRoot, 'views/layout/menu/navIndex.ts'), 'utf-8')
    const searchable = new Set<string>()
    for (const m of cmdText.matchAll(/\b(?:title|alias|subtitle): '([^']+)'/g)) {
      if (m[1]) searchable.add(m[1])
    }
    const orphan: string[] = []
    for (const f of readdirSafe(join(srcRoot, 'views/layout/menu/groups'))) {
      const text = readFileSync(f, 'utf-8')
      for (const m of text.matchAll(/\btitle: '([^']+)'/g)) {
        if (m[1] && !searchable.has(m[1])) orphan.push(`${m[1]}（${f.replace(/\\/g, '/')}）`)
      }
    }
    expect(searchable.size, '未从 navIndex.ts 解析到可搜索名，提取正则可能腐坏').toBeGreaterThanOrEqual(100)
    expect(
      orphan,
      `以下菜单标题在侧栏搜索搜不到，需统一命名或在 NavIndexItem.alias 登记菜单名：\n${orphan.join('\n')}`,
    ).toEqual([])
  })
})
