/**
 * Mock 层漂移守护元测试
 *
 * 背景：src/mock 与 src/api 是双轨数据源——页面先用 mock 演示，后端就绪后切到
 * api。历史问题：切换后旧 mock 无人引用、静默留在仓库里，与真实接口结构越走
 * 越远（双轨漂移）。
 *
 * 范围：本目录只保留「模拟后端」的模块（内存态 store / 种子数据 / 演示服务）。
 * 纯前端计算与校验工具不属于 mock，已迁往 src/utils（如 addrCalculator、
 * configFragment、batchImportCsv），其形状测试随模块放在 utils 对应 .spec 中。
 *
 * 本测试守护两条规则：
 *   1. 每个 mock 模块必须可加载且至少导出一个符号（防改名后引用方静默拿到 undefined）
 *   2. 每个 mock 文件必须被 src 下至少一处源码引用（孤儿 = 漂移产物，应当删除）
 *
 * 约定：mock 数据结构必须与对应 api/generated 类型保持一致；页面接入真实
 * api 后必须同步删除对应 mock 文件（本测试第 2 条会强制执行）。
 */
import { describe, it, expect } from 'vitest'
import { listUsers, listRoles } from './system/userPermissions'
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs'
import { join, dirname, resolve, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const srcRoot = resolve(here, '..')

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    let st
    try {
      st = statSync(p)
    } catch {
      continue
    }
    if (st.isDirectory()) walk(p, out)
    // .vue 单文件组件是 mock 的主要引用方，必须与 .ts 一并扫描，否则会产生误判
    else if ((name.endsWith('.ts') || name.endsWith('.vue')) && !name.endsWith('.spec.ts')) out.push(p)
  }
  return out
}

// 收集 src 下所有源码文本（排除 mock 自身、生成类型、spec）
function sourceFiles(): string[] {
  const out: string[] = []
  for (const sub of ['views', 'api', 'composables', 'components', 'stores', 'utils', 'router']) {
    const dir = join(srcRoot, sub)
    if (!existsSync(dir)) continue
    for (const f of walk(dir)) {
      if (f.includes('generated')) continue
      if (f.endsWith('.spec.ts')) continue
      out.push(f)
    }
  }
  return out
}

const texts = sourceFiles().map((f) => readFileSync(f, 'utf-8'))

const mockMods = import.meta.glob<Record<string, unknown>>('./**/*.ts', { eager: true })

describe('mock 层漂移守护', () => {
  it('每个 mock 模块可加载且至少导出一个符号', () => {
    const errs: string[] = []
    for (const [key, mod] of Object.entries(mockMods)) {
      if (key.endsWith('.spec.ts')) continue
      if (Object.keys(mod).length === 0) errs.push(`空模块（无任何导出）: ${key}`)
    }
    expect(errs, errs.join('\n')).toEqual([])
  })

  it('每个 mock 文件必须被 src 源码引用（孤儿即漂移产物）', () => {
    const orphans: string[] = []
    for (const f of walk(here)) {
      if (!f.endsWith('.ts')) continue // walk 现在也收 .vue，但 mock 层本身只统计 .ts
      if (f.endsWith('mocks.spec.ts')) continue
      // mock/config/commParams.ts → 引用片段 '/mock/config/commParams'
      const rel = relative(here, f).replace(/\.ts$/, '').split('\\').join('/')
      const frag = `/mock/${rel}`
      if (!texts.some((t) => t.includes(frag))) orphans.push(`孤儿 mock（无人引用）: src/mock/${rel}.ts`)
    }
    expect(orphans, orphans.join('\n')).toEqual([])
  })
})

describe('mock 形状冒烟（防重建漂移）', () => {
  it('userPermissions 用户与角色形状', async () => {
    const users = await listUsers()
    expect(users.length).toBeGreaterThanOrEqual(4)
    for (const u of users) {
      expect(['id', 'username', 'displayName', 'roleId', 'enabled', 'createdAt'].every((k) => k in u)).toBe(true)
    }
    const roles = await listRoles()
    expect(roles.length).toBeGreaterThanOrEqual(3)
  })
})
