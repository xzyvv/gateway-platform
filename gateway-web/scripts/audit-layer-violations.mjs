// 临时审计脚本：仅统计【静态 import】的分层倒挂，带 file:line
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

const layerOf = (r) => r.split('/')[0]
// 允许方向：低层 -> 高层（同层互允许）
const ALLOWED = {
  api: ['types', 'api', 'mock'],
  mock: ['types', 'mock'],
  types: ['types'],
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
for (const f of files) {
  const text = readFileSync(f, 'utf8')
  const lines = text.split(/\r?\n/)
  const withoutDyn = text.replace(/import\s*\(\s*['"][^'"]+['"]\s*\)/g, '""')
  // 逐行找静态 import，带行号
  const staticRe = /(?:import|export)[^;\n]*?from\s*['"]([^'"]+)['"]/g
  let m
  while ((m = staticRe.exec(withoutDyn))) {
    const spec = m[1]
    const target = resolveImport(spec, f)
    if (!target) continue
    const from = rel(f)
    const fromLayer = layerOf(from)
    const toLayer = layerOf(target)
    const allow = ALLOWED[fromLayer] || []
    if (allow.includes(toLayer)) continue
    if (fromLayer === toLayer) continue
    // 计算行号
    const before = withoutDyn.slice(0, m.index)
    const lineNo = before.split('\n').length
    const lineText = lines[lineNo - 1]?.trim() ?? ''
    violations.push({ from, lineNo, to: target, toLayer, spec, lineText })
  }
}

console.log(`=== 静态 import 分层倒挂: ${violations.length} 处 ===\n`)
const byKind = new Map()
for (const v of violations) {
  const k = `${v.from.split('/')[0]} -> ${v.toLayer}`
  if (!byKind.has(k)) byKind.set(k, [])
  byKind.get(k).push(v)
}
for (const [k, list] of [...byKind].sort((a, b) => b[1].length - a[1].length)) {
  console.log(`--- ${k}  (${list.length}) ---`)
  for (const v of list) {
    console.log(`  ${v.from}:${v.lineNo}  ->  ${v.to}`)
    console.log(`      ${v.lineText.slice(0, 110)}`)
  }
  console.log('')
}
