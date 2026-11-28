// 临时脚本：模拟「收窄 import 路径」后静态环会减少到多少（只读，不改源码）
// 假设的三处收窄：
//   N1  stores/auth.ts:3  from '@/api'        -> '@/api/token'
//   N2  stores/auth.ts:6  from '@/api/http'   -> '@/api/http/loggingOut'
//   N3  api/auth.ts:1     from './index'      -> './http'
//   N4  responseInterceptor.ts 的 3 个 store  -> 动态 import（等价于切断静态边）
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs'
import { join, dirname, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const src = join(root, 'src')

const files = []
function walk(dir) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) { if (e.name === 'generated') continue; walk(p) }
    else if (/\.(ts|vue)$/.test(e.name)) files.push(p)
  }
}
walk(src)
const rel = (p) => relative(src, p).split(sep).join('/')
const set = new Set(files.map(rel))

function resolveImport(spec, fromFile) {
  let base
  if (spec.startsWith('@/')) base = join(src, spec.slice(2))
  else if (spec.startsWith('.')) base = resolve(dirname(fromFile), spec)
  else return null
  for (const c of [base, base + '.ts', base + '.vue', join(base, 'index.ts')]) {
    if (existsSync(c) && statSync(c).isFile() && set.has(rel(c))) return rel(c)
  }
  return null
}

// 收窄规则：from 文件 -> 原始 spec -> 收窄后的 spec
const NARROW = {
  'stores/auth.ts': {
    '@/api': '@/api/token',
    '@/api/http': '@/api/http/loggingOut',
  },
  'api/auth.ts': {
    './index': './http',
  },
}
// 切断规则：from 文件 -> spec 前缀（该边视为动态，不参与静态环）
const CUT = {
  'api/http/responseInterceptor.ts': ['../../stores/notifications', '../../stores/requestLog', '../../stores/auth'],
}
// 只切断 useAuthStore 一条边（最小改动）
const CUT_AUTH_ONLY = {
  'api/http/responseInterceptor.ts': ['../../stores/auth'],
}

function buildEdges(applyNarrow, applyCut, cutMap = CUT) {
  const edges = new Map()
  for (const f of files) {
    const text = readFileSync(f, 'utf8')
    // 先剥掉块注释与行注释——否则 JSDoc 里举例用的 import 语句会被误当成真实依赖
    // （例：components/dataTable/types.ts:9 的注释里含 `import type { ColumnDef } from '@/components/DataTable.vue'`）
    const stripped = text
      .replace(/\/\*[\s\S]*?\*\//g, '""')
      .replace(/^\s*\/\/.*$/gm, '""')
      .replace(/import\s*\(\s*['"][^'"]+['"]\s*\)/g, '""')
    const st = new Set()
    const re = /(?:^|[\s;}])(?:import|export)[^;\n]*?from\s*['"]([^'"]+)['"]/g
    let m
    while ((m = re.exec(stripped))) {
      let spec = m[1]
      const from = rel(f)
      if (applyCut && cutMap[from]?.some((p) => spec.endsWith(p))) continue
      if (applyNarrow && NARROW[from]?.[spec]) spec = NARROW[from][spec]
      const t = resolveImport(spec, f)
      if (t && t !== from) st.add(t)
    }
    // 副作用裸导入（import './responseInterceptor'）——同样是静态依赖边
    const bareRe = /(?:^|[\s;}])import\s+['"]([^'"]+)['"]/g
    while ((m = bareRe.exec(stripped))) {
      const t = resolveImport(m[1], f)
      if (t && t !== rel(f)) st.add(t)
    }
    edges.set(rel(f), st)
  }
  return edges
}

function isBarrel(r) {
  if (!/(^|\/)index\.ts$/.test(r)) return false
  const text = readFileSync(join(src, r), 'utf8')
  return [...text.matchAll(/export\s+(?!from|\{)/g)].length === 0
}

function findCycles(edges) {
  const cycles = []
  const seen = new Set()
  function dfs(n, path, inStack) {
    if (inStack.has(n)) {
      const i = [...inStack].indexOf(n)
      const cyc = [...path.slice(i), n]
      const stripped = cyc.filter((x) => !isBarrel(x))
      if (stripped.length > 2) {
        const key = stripped.slice(0, -1).sort().join('|')
        if (!seen.has(key)) { seen.add(key); cycles.push(stripped) }
      }
      return
    }
    const nextStack = new Set([...inStack, n])
    for (const d of edges.get(n) || []) dfs(d, [...path, n], nextStack)
  }
  for (const n of edges.keys()) dfs(n, [], new Set())
  return cycles
}

const base = findCycles(buildEdges(false, false))
console.log(`=== 基线静态环: ${base.length} 组 ===`)
for (const c of base) console.log('  ' + c.join(' ⟶ '))

const s1 = findCycles(buildEdges(true, false))
console.log(`\n=== 仅收窄 import（N1+N2+N3）: ${s1.length} 组 ===`)
for (const c of s1) console.log('  ' + c.join(' ⟶ '))

const s2 = findCycles(buildEdges(true, true))
console.log(`\n=== 收窄 + 切断 responseInterceptor 的 3 个 store 静态边: ${s2.length} 组 ===`)
for (const c of s2) console.log('  ' + c.join(' ⟶ '))

const s3 = findCycles(buildEdges(false, true))
console.log(`\n=== 仅切断（不收窄）: ${s3.length} 组 ===`)
for (const c of s3) console.log('  ' + c.join(' ⟶ '))

const s4 = findCycles(buildEdges(false, true, CUT_AUTH_ONLY))
console.log(`\n=== 只切断 responseInterceptor -> stores/auth 一条边: ${s4.length} 组 ===`)
for (const c of s4) console.log('  ' + c.join(' ⟶ '))

const s5 = findCycles(buildEdges(true, true, CUT_AUTH_ONLY))
console.log(`\n=== 收窄 + 只切断 stores/auth 一条边: ${s5.length} 组 ===`)
for (const c of s5) console.log('  ' + c.join(' ⟶ '))
