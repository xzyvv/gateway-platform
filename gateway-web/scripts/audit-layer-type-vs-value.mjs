// 临时审计脚本：把分层倒挂按「类型导入 vs 值导入」区分开
// 类型导入在编译期擦除 → 不会造成运行时环；值导入才会。
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

const valueViolations = []
const typeViolations = []

for (const f of files) {
  const text = readFileSync(f, 'utf8')
  const lines = text.split(/\r?\n/)
  const withoutDyn = text.replace(/import\s*\(\s*['"][^'"]+['"]\s*\)/g, '""')
  const staticRe = /(?:import|export)[^;\n]*?from\s*['"]([^'"]+)['"]/g
  let m
  while ((m = staticRe.exec(withoutDyn))) {
    const spec = m[1]
    const target = resolveImport(spec, f)
    if (!target) continue
    const from = rel(f)
    const fromLayer = layerOf(from)
    const toLayer = layerOf(target)
    if (fromLayer === toLayer) continue
    if ((ALLOWED[fromLayer] || []).includes(toLayer)) continue
    const stmt = m[0]
    const isTypeOnly = /import\s+type\b/.test(stmt)
    const lineNo = withoutDyn.slice(0, m.index).split('\n').length
    const rec = { from, lineNo, to: target, toLayer, stmt: stmt.replace(/\s+/g, ' ').trim(), line: lines[lineNo - 1]?.trim() ?? '' }
    if (isTypeOnly) typeViolations.push(rec)
    else valueViolations.push(rec)
  }
}

console.log(`=== 值导入型分层倒挂（真实运行时倒挂）: ${valueViolations.length} 处 ===\n`)
for (const v of valueViolations) {
  console.log(`  ${v.from}:${v.lineNo}  ->  ${v.to}   [${layerOf(v.from)} -> ${v.toLayer}]`)
  console.log(`      ${v.line.slice(0, 120)}`)
}
console.log(`\n=== 类型导入型倒挂（编译期擦除，无运行时环）: ${typeViolations.length} 处 ===`)
const byPair = new Map()
for (const t of typeViolations) {
  const k = `${layerOf(t.from)} -> ${t.toLayer}`
  if (!byPair.has(k)) byPair.set(k, [])
  byPair.get(k).push(t)
}
for (const [k, list] of byPair) console.log(`  ${k}: ${list.length} 处`)
console.log('\n（明细）')
for (const t of typeViolations) console.log(`  ${t.from}:${t.lineNo}  ->  ${t.to}`)
