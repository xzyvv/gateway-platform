#!/usr/bin/env node
/**
 * 前端产物体积门禁（零依赖，CI 与本地均可运行）。
 *
 * 指标（2026-10 调整）：
 * - 首屏闭合集：dist/index.html 直接引用的 script / modulepreload / stylesheet。
 *   打开任意页面都必须先下载完这批文件，它才代表用户真实等待的时间 → 硬预算。
 * - 全量 gzip：所有 js/css 之和。其中 150+ 个 chunk 按路由懒加载，
 *   单次会话几乎不会全部加载，因此不再作为失败条件，只作观察与告警。
 * - 单个 chunk：保留硬预算，防止某个页面把大依赖塞进自己的 chunk。
 *
 * 旧口径（全量 1100KB）的问题：预算只剩 3% 余量，且约束的是一个用户永远不会
 * 一次性访问的数字，新增页面时会逼着做错误的拆包决策。
 *
 * 预算可用环境变量覆盖：
 *   BUNDLE_FIRST_LOAD_KB  首屏闭合集 gzip 预算（默认 190；2026-10 基线约 158）
 *   BUNDLE_CHUNK_KB       单个 chunk gzip 预算（默认 350）
 *   BUNDLE_TOTAL_WARN_KB  全量 gzip 观察阈值（默认 1400，仅告警不失败）
 *
 * 用法：npm run check:bundle
 */
import { readdirSync, readFileSync } from 'node:fs'
import { gzipSync } from 'node:zlib'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const distDir = join(here, '..', 'dist')
const distAssets = join(distDir, 'assets')

const FIRST_LOAD_BUDGET_KB = Number(process.env.BUNDLE_FIRST_LOAD_KB ?? 190)
const CHUNK_BUDGET_KB = Number(process.env.BUNDLE_CHUNK_KB ?? 350)
const TOTAL_WARN_KB = Number(process.env.BUNDLE_TOTAL_WARN_KB ?? 1400)

const kb = (n) => (n / 1024).toFixed(1)

let files
try {
  files = readdirSync(distAssets).filter((f) => /\.(js|css)$/.test(f))
} catch {
  console.error(`[bundle] 未找到 ${distAssets}，请先运行 npm run build`)
  process.exit(2)
}
if (files.length === 0) {
  console.error(`[bundle] ${distAssets} 下没有 js/css 产物`)
  process.exit(2)
}

const sizes = new Map(
  files.map((f) => {
    const buf = readFileSync(join(distAssets, f))
    return [f, { raw: buf.length, gzip: gzipSync(buf).length }]
  }),
)

// ── 首屏闭合集：以构建产物里的 index.html 为准，而不是手写白名单 ──────────
let html
try {
  html = readFileSync(join(distDir, 'index.html'), 'utf8')
} catch {
  console.error('[bundle] 未找到 dist/index.html，无法计算首屏闭合集')
  process.exit(2)
}
const entryFiles = [
  ...new Set(
    [...html.matchAll(/(?:src|href)="\/assets\/([^"]+\.(?:js|css))"/g)].map((m) => m[1]),
  ),
]
if (entryFiles.length === 0) {
  console.error('[bundle] dist/index.html 中没有解析到任何 /assets/ 引用')
  process.exit(2)
}

const rows = [...sizes.entries()]
  .map(([f, s]) => ({ f, ...s }))
  .sort((a, b) => b.gzip - a.gzip)
const entryRows = entryFiles
  .map((f) => rows.find((r) => r.f === f))
  .filter((r) => r !== undefined)

const missing = entryFiles.filter((f) => !sizes.has(f))
const firstLoadGzip = entryRows.reduce((s, r) => s + r.gzip, 0)
const totalGzip = rows.reduce((s, r) => s + r.gzip, 0)
const lazyGzip = totalGzip - firstLoadGzip

console.log('[bundle] gzip 体积 top 10：')
for (const r of rows.slice(0, 10)) {
  console.log(`  ${kb(r.gzip).padStart(9)} KB gz  ${kb(r.raw).padStart(9)} KB raw  ${r.f}`)
}
console.log(
  `[bundle] 首屏闭合集 ${entryRows.length} 个文件，gzip ${kb(firstLoadGzip)} KB ` +
    `（预算 ${FIRST_LOAD_BUDGET_KB} KB）`,
)
console.log(`[bundle] 懒加载 ${rows.length - entryRows.length} 个文件，gzip ${kb(lazyGzip)} KB`)
console.log(
  `[bundle] 全量 ${rows.length} 个文件，gzip ${kb(totalGzip)} KB ` +
    `（观察阈值 ${TOTAL_WARN_KB} KB，仅告警）`,
)

const errors = []
if (missing.length > 0) {
  errors.push(`index.html 引用了不存在的产物：${missing.join(', ')}`)
}
if (firstLoadGzip > FIRST_LOAD_BUDGET_KB * 1024) {
  errors.push(
    `首屏闭合集 gzip ${kb(firstLoadGzip)} KB 超过预算 ${FIRST_LOAD_BUDGET_KB} KB`,
  )
}
for (const r of rows) {
  if (r.gzip > CHUNK_BUDGET_KB * 1024) {
    errors.push(`单块 ${r.f} gzip ${kb(r.gzip)} KB 超过 ${CHUNK_BUDGET_KB} KB`)
  }
}
if (errors.length > 0) {
  for (const e of errors) console.error(`[bundle] ✗ ${e}`)
  process.exit(1)
}
if (totalGzip > TOTAL_WARN_KB * 1024) {
  console.warn(
    `[bundle] ⚠ 全量 gzip ${kb(totalGzip)} KB 超过观察阈值 ${TOTAL_WARN_KB} KB：` +
      '请确认是否新增了重复依赖；该指标不阻塞构建',
  )
}
console.log('[bundle] ✓ 首屏与单块预算通过')
