import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 展示型组件单一实现守护。
 *
 * 背景（W2-#11）：空态与统计卡长期存在"组件 + 页面手写一套"的双轨。
 * 手写版一旦漂移，同一界面会出现两种空态/统计卡观感，且新页面没有明确范式。
 * 这里用静态扫描把两条轨道收成一条：新页面必须复用组件，不允许再手写。
 */
describe('展示组件单一实现守护', () => {
  const here = dirname(fileURLToPath(import.meta.url))
  const srcRoot = resolve(here, '..')

  function walkVue(dir: string, out: string[] = []): string[] {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name)
      let st
      try {
        st = statSync(p)
      } catch {
        continue
      }
      if (st.isDirectory()) out = walkVue(p, out)
      else if (name.endsWith('.vue')) out.push(p)
    }
    return out
  }

  function offendersOf(pattern: RegExp, allowlist: string[]): string[] {
    const allow = new Set(allowlist)
    return walkVue(srcRoot)
      .map((f) => `src/${relative(srcRoot, f).split('\\').join('/')}`)
      .filter((rel) => !allow.has(rel) && pattern.test(readFileSync(join(srcRoot, rel.replace('src/', '')), 'utf-8')))
  }

  it('空态统一走 EmptyState（禁止页面直接写 el-empty）', () => {
    // 白名单：EmptyState 自身就是对 el-empty 的封装
    expect(offendersOf(/<el-empty[\s>]/, ['src/components/EmptyState.vue'])).toEqual([])
  })

  it('统计卡统一走 StatCard（禁止页面手写 class="stat-card" 卡片）', () => {
    // 白名单：StatCard 自身；骨架占位用 sk-stat-card，不命中该正则
    expect(offendersOf(/class="stat-card"/, ['src/components/StatCard.vue'])).toEqual([])
  })

  it('表格骨架统一走 TableSkeleton（禁止页面手写表格行占位）', () => {
    // 白名单：TableSkeleton 自身。
    // 只扫 sk-cell 不扫 sk-row：CellularStatus 把 sk-row 用作键值块占位（非表格位），
    // 卡片 / 图表 / 代码 / diff 等非表格位置允许直接使用 el-skeleton。
    expect(offendersOf(/class="sk-cell"/, ['src/components/TableSkeleton.vue'])).toEqual([])
  })

  it('筛选条统一走 FilterToolbar（禁止页面再拼一套筛选条外壳）', () => {
    // 白名单：FilterToolbar 自身。复制它的外壳类名等于复制实现，同样收口。
    expect(offendersOf(/class="filter-toolbar"/, ['src/components/FilterToolbar.vue'])).toEqual([])
  })
})
