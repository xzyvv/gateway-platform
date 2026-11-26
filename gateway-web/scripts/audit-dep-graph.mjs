// 临时审计脚本：构建 src/ 内部依赖图，检测循环依赖、孤儿模块、分层倒挂。
// 用法: node scripts/audit-dep-graph.mjs
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs'
import { join, dirname, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const src = join(root, 'src')
const alias = '@/' // vite alias -> src/

const files = []
function walk(dir) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) {
      if (e.name === 'generated') continue
      walk(p)
    } else if (/\.(ts|vue)$/.test(e.name)) {
      files.push(p)
    }
  }
}
walk(src)

const rel = (p) => relative(src, p).split(sep).join('/')
const isSpec = (p) => /\.(spec|test)\.ts$/.test(p)
const set = new Set(files.map(rel))

function resolveImport(spec, fromFile) {
  let base
  if (spec.startsWith(alias)) base = join(src, spec.slice(alias.length))
  else if (spec.startsWith('.')) base = resolve(dirname(fromFile), spec)
  else return null // bare package
  const cands = [base, base + '.ts', base + '.vue', base + '.index.ts', join(base, 'index.ts')]
  for (const c of cands) {
    if (existsSync(c) && statSync(c).isFile()) {
      const r = rel(c)
      if (set.has(r)) return r
    }
  }
  return 'UNRESOLVED:' + spec
}

const graph = new Map() // rel -> Set(rel)
const unresolved = []
for (const f of files) {
  const text = readFileSync(f, 'utf8')
  const deps = new Set()
  const re = /(?:from\s*|import\s*\(\s*|import\s+)['"]([^'"]+)['"]/g
  let m
  while ((m = re.exec(text))) {
    const target = resolveImport(m[1], f)
    if (target === null) continue
    if (target.startsWith('UNRESOLVED:')) {
      unresolved.push(`${rel(f)} -> ${m[1]}`)
      continue
    }
    if (target !== rel(f)) deps.add(target)
  }
  graph.set(rel(f), deps)
}

// ---- 1. 循环依赖 ----
const cycles = []
const state = new Map() // 0 unvisited,1 in stack,2 done
const stack = []
function dfs(n) {
  state.set(n, 1)
  stack.push(n)
  for (const d of graph.get(n) || []) {
    if (!graph.has(d)) continue
    if (state.get(d) === 1) {
      const i = stack.indexOf(d)
      cycles.push([...stack.slice(i), d])
    } else if (!state.get(d)) dfs(d)
  }
  stack.pop()
  state.set(n, 2)
}
for (const n of graph.keys()) if (!state.get(n)) dfs(n)

// ---- 2. 反向依赖（被谁引用）----
const reverse = new Map()
for (const [n, deps] of graph) {
  for (const d of deps) {
    if (!reverse.has(d)) reverse.set(d, new Set())
    reverse.get(d).add(n)
  }
}

// ---- 3. 分层规则 ----
const LAYERS = ['api', 'components', 'composables', 'mock', 'router', 'stores', 'types', 'utils', 'views', 'app', 'assets']
const layerOf = (r) => {
  const seg = r.split('/')
  return seg.length > 1 && LAYERS.includes(seg[0]) ? seg[0] : seg[0]
}
// 允许的依赖方向（由低到高）：api/types/mock <- utils <- stores <- composables <- components <- views
const ALLOWED = {
  api: ['types', 'api', 'mock'],
  types: ['types'],
  mock: ['types', 'mock'],
  utils: ['types', 'utils', 'api'],
  stores: ['types', 'utils', 'api', 'stores'],
  composables: ['types', 'utils', 'api', 'stores', 'composables'],
  components: ['types', 'utils', 'api', 'stores', 'composables', 'components'],
  views: ['types', 'utils', 'api', 'stores', 'composables', 'components', 'views'],
  router: ['types', 'utils', 'api', 'stores', 'components', 'views'],
  app: ['types', 'utils', 'api', 'stores', 'composables', 'components', 'views', 'app'],
  assets: ['assets'],
}
const violations = []
for (const [n, deps] of graph) {
  const ln = layerOf(n)
  const allow = ALLOWED[ln] || []
  for (const d of deps) {
    const ld = layerOf(d)
    if (!allow.includes(ld)) {
      violations.push({ from: n, to: d, fromLayer: ln, toLayer: ld })
    }
  }
}

// ---- 4. 孤儿模块（无任何非 spec 引用者）----
const orphans = []
for (const n of graph.keys()) {
  if (isSpec(n)) continue
  const refs = reverse.get(n)
  if (!refs || refs.size === 0) {
    // 入口点豁免
    if (/^(app\/main\.ts|router\/index\.ts)$/.test(n)) continue
    orphans.push(n)
  }
}

// ---- 5. 只被 spec 引用的模块（生产代码里没人用）----
const specOnly = []
for (const n of graph.keys()) {
  if (isSpec(n)) continue
  const refs = reverse.get(n)
  if (!refs) continue
  const nonSpecRefs = [...refs].filter((r) => !isSpec(r))
  if (nonSpecRefs.length === 0) specOnly.push(n)
}

// ---- 输出 ----
console.log(`=== 文件总数: ${files.length} ===`)
console.log(`\n=== 循环依赖: ${cycles.length} 组 ===`)
for (const c of cycles) console.log('  ' + c.join(' -> '))

console.log(`\n=== 分层倒挂: ${violations.length} 处 ===`)
for (const v of violations) console.log(`  ${v.from}  [${v.fromLayer}] --> ${v.to}  [${v.toLayer}]`)

console.log(`\n=== 孤儿模块（无引用）: ${orphans.length} 个 ===`)
for (const o of orphans) console.log('  ' + o)

console.log(`\n=== 仅被 spec 引用: ${specOnly.length} 个 ===`)
for (const o of specOnly) console.log('  ' + o)

console.log(`\n=== 未解析导入: ${unresolved.length} 处 ===`)
for (const u of unresolved.slice(0, 40)) console.log('  ' + u)
