// 临时审计脚本：区分「静态 import」与「动态 import()」的真实循环依赖
// 动态 import 常被刻意用来打断环（项目里已有注释说明），静态环才是硬债。
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
const isSpec = (r) => /\.(spec|test)\.ts$/.test(r)

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

// staticEdges[a] -> b : 静态 import（真依赖，参与环计算）
// dynamicEdges[a] -> b : 动态 import()（通常是刻意断环手段）
const staticEdges = new Map()
const dynamicEdges = new Map()
for (const f of files) {
  const text = readFileSync(f, 'utf8')
  const st = new Set(), dy = new Set()
  // 动态：import('...')
  let m
  const dynRe = /import\s*\(\s*['"]([^'"]+)['"]\s*\)/g
  while ((m = dynRe.exec(text))) {
    const t = resolveImport(m[1], f)
    if (t && t !== rel(f)) dy.add(t)
  }
  // 静态：排除 import(...) 后再匹配 from '...'
  const withoutDyn = text.replace(/import\s*\(\s*['"][^'"]+['"]\s*\)/g, '""')
  const staticRe = /(?:^|[\s;}])(?:import|export)[^;\n]*?from\s*['"]([^'"]+)['"]/g
  while ((m = staticRe.exec(withoutDyn))) {
    const t = resolveImport(m[1], f)
    if (t && t !== rel(f)) st.add(t)
  }
  // import '...' 裸副作用导入
  const bareRe = /(?:^|[\s;}])import\s+['"]([^'"]+)['"]/g
  while ((m = bareRe.exec(withoutDyn))) {
    const t = resolveImport(m[1], f)
    if (t && t !== rel(f)) st.add(t)
  }
  staticEdges.set(rel(f), st)
  dynamicEdges.set(rel(f), dy)
}

// barrel 检测：index.ts 只做 re-export
function isBarrel(r) {
  if (!/(^|\/)index\.ts$/.test(r)) return false
  const text = readFileSync(join(src, r), 'utf8')
  const bare = [...text.matchAll(/export\s+(?!from|\{)/g)].length
  return bare === 0
}

// 找静态环
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
  for (const d of staticEdges.get(n) || []) dfs(d, [...path, n], nextStack)
}
for (const n of staticEdges.keys()) dfs(n, [], new Set())

console.log(`=== 静态 import 循环依赖（剥离 barrel 后）: ${cycles.length} 组 ===`)
for (const c of cycles) console.log('  ' + c.join('  ⟶  '))

// 统计每个节点作为「环枢纽」的次数
const hubCount = new Map()
for (const c of cycles) {
  for (const n of c.slice(0, -1)) hubCount.set(n, (hubCount.get(n) || 0) + 1)
}
console.log('\n=== 环枢纽排行（出现在多少个环里）===')
for (const [n, k] of [...hubCount].sort((a, b) => b[1] - a[1])) {
  console.log(`  ${k}x  ${n}`)
}

// 动态 import 的断环使用统计
console.log('\n=== 动态 import() 使用（断环手段）===')
let dynCount = 0
for (const [n, ds] of dynamicEdges) {
  if (ds.size) {
    dynCount += ds.size
    console.log(`  ${n}  ->  ${[...ds].join(', ')}`)
  }
}
console.log(`\n动态 import 总数: ${dynCount}`)

// 静态环里是否有 spec 文件参与
console.log('\n=== 参与静态环的 spec 文件（测试依赖生产代码方向异常时可见）===')
for (const c of cycles) {
  const specNodes = c.filter((n) => isSpec(n))
  if (specNodes.length) console.log(`  ${c.join(' ⟶ ')}`)
}
