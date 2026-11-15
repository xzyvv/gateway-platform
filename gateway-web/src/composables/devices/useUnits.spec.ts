import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { unitOptions, unitChoices } from './useUnits'

/**
 * 单位下拉的两条不变量：
 * 1) 清单必须覆盖仓库内置模板里真实出现的单位，否则模板建出来的点位在下拉里显示空白；
 * 2) 存量配置可能含清单外的旧写法，下拉要能把当前值补进选项，避免老点位被静默改写。
 */

const TEMPLATE_DIR = join(__dirname, '../../../templates')

function templateUnitValues(): string[] {
  const out: string[] = []
  for (const sub of ['builtin', 'custom']) {
    const dir = join(TEMPLATE_DIR, sub)
    let files: string[] = []
    try {
      files = readdirSync(dir).filter((f) => f.endsWith('.json'))
    } catch {
      continue
    }
    for (const f of files) {
      const raw = readFileSync(join(dir, f), 'utf-8')
      for (const m of raw.matchAll(/"unit"\s*:\s*"([^"]*)"/g)) {
        if (m[1]) out.push(m[1])
      }
    }
  }
  return out
}

describe('useUnits 单位下拉', () => {
  it('清单覆盖内置模板里实际出现的所有单位', () => {
    const missing = [...new Set(templateUnitValues())].filter((u) => !unitOptions.includes(u))
    expect(missing).toEqual([])
  })

  it('存量值不在清单时补进选项，保持可选可见', () => {
    expect(unitChoices('℃')).toContain('℃')
    expect(unitChoices('m³')).toContain('m³')
    expect(unitChoices('kWh').filter((u) => u === 'kWh')).toHaveLength(1)
  })

  it('空值 / 清单内值不产生额外选项', () => {
    expect(unitChoices('')).toEqual(unitOptions)
    expect(unitChoices(null)).toEqual(unitOptions)
    expect(unitChoices('  ')).toEqual(unitOptions)
  })
})
