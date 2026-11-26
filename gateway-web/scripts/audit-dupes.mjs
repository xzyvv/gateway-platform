// 临时审计脚本：剥离 barrel 后的真实循环依赖 + 近似重复实现检测
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

const graph = new Map()
for (const f of files) {
  const text = readFileSync(f, 'utf8')
  const deps = new Set()
  const re = /(?:from\s*|import\s*\(\s*)['"]([^'"]+)['"]/g
  let m
  while ((m = re.exec(text))) {
    const t = resolveImport(m[1], f)
    if (t && t !== rel(f)) deps.add(t)
  }
  graph.set(rel(f), deps)
}

// ---- 识别 barrel：只做 re-export（无其他 export）的 index 文件 ----
function isBarrel(r) {
  if (!/(^|\/)index\.ts$/.test(r)) return false
  const text = readFileSync(join(src, r), 'utf8')
  // 允许仅有 `export * from` / `export {x} from`
  const bare = [...text.matchAll(/export\s+(?!from|{)/g)].length
  return bare === 0
}

// ---- 真实循环：忽略 barrel 节点（barrel 只是转发，剥离后仍有环才算债）----
const realCycles = []
const seen = new Set()
function dfs(n, path, inStack) {
  if (inStack.has(n)) {
    const i = [...inStack].indexOf(n)
    const cyc = [...path.slice(i), n]
    // 剥离 barrel 节点
    const stripped = cyc.filter((x) => !isBarrel(x))
    if (stripped.length > 2) {
      const key = stripped.slice(0, -1).sort().join('|')
      if (!seen.has(key)) { seen.add(key); realCycles.push(stripped) }
    }
    return
  }
  const nextStack = new Set([...inStack, n])
  for (const d of graph.get(n) || []) dfs(d, [...path, n], nextStack)
}
for (const n of graph.keys()) dfs(n, [], new Set())

console.log(`=== 剥离 barrel 后的真实循环依赖: ${realCycles.length} 组 ===`)
for (const c of realCycles) console.log('  ' + c.join('  ⟶  '))

// ---- 近似重复实现：按「函数体归一化指纹」分组 ----
function fingerprint(text) {
  // 提取函数体，压缩空白与变量名差异
  return text
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\/\/.*$/gm, '')
    .replace(/\s+/g, ' ')
    .trim()
}
const bodies = new Map()
const fnRe = /(?:export\s+)?(?:async\s+)?function\s+([A-Za-z0-9_]+)\s*\([^)]*\)\s*(?::[^{]+)?\{([\s\S]*?)\n\}/g
for (const f of files) {
  if (/\.(spec|test)\.ts$/.test(f)) continue
  const text = readFileSync(f, 'utf8')
  let m
  while ((m = fnRe.exec(text))) {
    const name = m[1]
    const body = fingerprint(m[2])
    if (body.length < 120) continue // 跳过太短的
    if (!bodies.has(name)) bodies.set(name, [])
    bodies.get(name).push({ file: rel(f), body, len: body.length })
  }
}
console.log('\n=== 同名函数体长度高度接近（疑似重复实现）===')
for (const [name, list] of bodies) {
  if (list.length < 2) continue
  const uniqFiles = new Set(list.map((x) => x.file))
  if (uniqFiles.size < 2) continue
  // 两两比较归一化后相似度（去掉标识符后的字符集）
  const norm = (s) => s.replace(/\b[A-Za-z_$][A-Za-z0-9_$]*\b/g, 'X')
  for (let i = 0; i < list.length; i++) {
    for (let j = i + 1; j < list.length; j++) {
      if (list[i].file === list[j].file) continue
      const a = norm(list[i].body), b = norm(list[j].body)
      const la = a.length, lb = b.length
      if (Math.abs(la - lb) / Math.max(la, lb) > 0.25) continue
      // 词级 Jaccard
      const wa = new Set(a.split(/[^A-Za-z0-9]+/).filter(Boolean))
      const wb = new Set(b.split(/[^A-Za-z0-9]+/).filter(Boolean))
      let inter = 0
      for (const w of wa) if (wb.has(w)) inter++
      const union = wa.size + wb.size - inter
      const sim = union ? inter / union : 0
      if (sim >= 0.85) {
        console.log(`  ~${(sim * 100).toFixed(0)}% ${name}`)
        console.log(`      ${list[i].file}  (${list[i].len} 字)`)
        console.log(`      ${list[j].file}  (${list[j].len} 字)`)
      }
    }
  }
}
