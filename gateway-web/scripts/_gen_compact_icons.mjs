/**
 * 从 @element-plus/icons-vue/dist/index.js 提取所有图标的 SVG path 数据，
 * 生成紧凑的图标模块 src/utils/ep-icons-compact.ts。
 *
 * 紧凑版去掉了每个图标独立的 defineComponent / name / __name / setup 包装，
 * 改用一个共享的 ic() 工厂函数，每个图标仅保留路径数据 + 一次工厂调用。
 */
import { readFileSync, writeFileSync } from 'node:fs'

const src = readFileSync('node_modules/@element-plus/icons-vue/dist/index.js', 'utf8')

// 每个图标块以 // src/components/xxx.vue 开头，到下一个 // 注释或文件末尾
// 块内含 name: "PascalName" 和若干 d: "path"
const blocks = src.split(/\/\/ src\/components\//).slice(1)

/**
 * SVG path 数字 token 的形态（path 语法允许省略前导 0 与分隔符）：
 *   [+-]? ( \d+\.?\d* | \.\d+ ) ( [eE][+-]?\d+ )?
 */
const NUMBER_RE = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/

/** 把 path 字符串拆成命令字母与数字 token 交替的序列 */
function tokenize(d) {
  const tokens = []
  let i = 0
  while (i < d.length) {
    const ch = d[i]
    if (ch === ' ' || ch === ',' || ch === '\t' || ch === '\n' || ch === '\r') {
      i++
      continue
    }
    if (/[A-Za-z]/.test(ch)) {
      tokens.push({ cmd: ch })
      i++
      continue
    }
    const m = NUMBER_RE.exec(d.slice(i))
    if (!m) throw new Error(`无法解析 path（位置 ${i}）: …${d.slice(Math.max(0, i - 16), i + 16)}…`)
    tokens.push({ num: m[0] })
    i += m[0].length
  }
  return tokens
}

/**
 * 把 path 中的每个数值坐标四舍五入到整数，再安全地重新序列化。
 *
 * 必须是「先分词再取值」，不能对原始字符串跑正则替换：
 * path 语法里相邻两个数可以完全不带分隔符，例如 `-5.376.32` 是
 * 「-5.376」和「.32」两个数。若用 /\d+\.\d+/g 直接替换，`5.376` 变成 `5`，
 * 紧邻的 `.32` 就粘成 `5.32`——一个数凭空消失，后续 cubic 的 6 个参数
 * 只剩 5 个，整条 path 语法非法，浏览器抛
 * `<path> attribute d: Expected number`。
 *
 * viewBox 是 0 0 1024 1024，取整最大误差 0.5/1024 = 0.05%，16px 图标下
 * 误差 <0.01px，肉眼完全不可感知，但能省约 24% 的 path 数据体积。
 */
function compactPath(d) {
  const tokens = tokenize(d)
  const out = []
  for (const t of tokens) {
    if (t.cmd !== undefined) {
      out.push({ s: t.cmd, isNum: false })
      continue
    }
    const n = Math.round(parseFloat(t.num))
    if (!Number.isFinite(n)) throw new Error(`非法数值 "${t.num}"`)
    out.push({ s: String(n === 0 ? 0 : n), isNum: true })
  }
  // 序列化：命令字母后直接跟数字；两个数字相邻时，只有后一个以 '-' 开头
  // 才可省略空格（负号本身起分隔作用），其余情况必须补空格，否则会粘连。
  let result = ''
  let prevIsNum = false
  for (const { s, isNum } of out) {
    if (prevIsNum && isNum && !s.startsWith('-')) result += ' '
    result += s
    prevIsNum = isNum
  }
  return result
}

/** 移除数字与命令字母之间的多余空格（SVG path 语法允许省略） */
function stripSpaces(d) {
  return d.replace(/(\d)\s+([a-zA-Z])/g, '$1$2')
}

const icons = []
for (const block of blocks) {
  const nameMatch = block.match(/name:\s*"([A-Za-z]+)"/)
  if (!nameMatch) continue
  const name = nameMatch[1]
  const paths = [...block.matchAll(/d:\s*"([^"]+)"/g)].map((m) => stripSpaces(compactPath(m[1])))
  if (paths.length === 0) continue
  icons.push({ name, paths })
}

console.log(`Extracted ${icons.length} icons (coordinates rounded to int)`)

const lines = []
lines.push('/**')
lines.push(' * 紧凑版 Element Plus 图标（自动生成，勿手改）。')
lines.push(' *')
lines.push(' * 优化点：')
lines.push(' * 1. 去掉每个图标独立的 defineComponent / name / __name / setup 包装，')
lines.push(' *    改用共享工厂 ic()，每图标仅存 path 数据。')
lines.push(' * 2. path 坐标四舍五入到整数（viewBox 1024，误差 0.05%，肉眼不可感知），')
lines.push(' *    再省约 24% 的 path 数据体积。')
lines.push(' * 3. 移除数字与命令字母之间的多余空格（SVG path 语法允许）。')
lines.push(' * 通过 vite.config.ts 的 resolve.alias 把 "@element-plus/icons-vue"')
lines.push(' * 重定向到此模块。')
lines.push(' */')
lines.push("import { h, type Component } from 'vue'")
lines.push('')
lines.push("const SVG_ATTRS = { xmlns: 'http://www.w3.org/2000/svg', viewBox: '0 0 1024 1024' }")
lines.push('')
lines.push('/** 图标工厂：path 为字符串或字符串数组（多 path 图标） */')
lines.push('function ic(d: string | string[]): Component {')
lines.push('  const arr = Array.isArray(d) ? d : [d]')
lines.push('  return {')
lines.push('    setup() {')
lines.push("      return () => h('svg', SVG_ATTRS, arr.map((p) => h('path', { fill: 'currentColor', d: p })))")
lines.push('    },')
lines.push('  }')
lines.push('}')
lines.push('')

for (const { name, paths } of icons) {
  const arg = paths.length === 1 ? JSON.stringify(paths[0]) : JSON.stringify(paths)
  lines.push(`export const ${name} = ic(${arg})`)
}

lines.push('')
writeFileSync('src/utils/ep-icons-compact.ts', lines.join('\n'))
console.log(`Wrote src/utils/ep-icons-compact.ts (${lines.length} lines)`)
