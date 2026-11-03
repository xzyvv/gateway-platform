/**
 * 守护测试：Hub 内的每个页签都必须能被侧栏搜索按页签文字搜到（P2 #2）。
 *
 * 历史问题：Hub 页签共 96 个，而搜索索引只登记了部分入口。搜索只匹配
 * title / subtitle / alias，因此「索引指向了该 tab，但标题与页签标签不同名」
 * 同样等于搜不到——现场表现为：用户看着页签上的字（如「设备清单」）敲进搜索框，
 * 结果空空如也，只能靠鼠标在多级菜单里翻找。
 *
 * 本测试从各 Hub 源文件里解析出真实的页签定义（name + label），
 * 逐个断言存在一条索引项能按该 label 搜到并跳转到对应路径。
 */

import { describe, it, expect } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { navIndexItems } from './navIndex'

interface HubTab {
  /** Hub 路由前缀 */
  route: string
  /** 页签内部 name（对应 ?tab= 参数） */
  name: string
  /** 页签上显示的文字（用户会照着这个字搜索） */
  label: string
}

/** Hub 源文件 → 路由前缀（16 → 10：system / devices / points / data / ops 各合并同域 Hub） */
const HUB_ROUTES: Record<string, string> = {
  'system/SystemHub.vue': '/system',
  'system/config/ConfigHub.vue': '/system/config',
  'devices/DeviceHub.vue': '/devices',
  'points/PointHub.vue': '/points',
  'acquisition/ProtocolConsoleHub.vue': '/acquisition/console',
  'acquisition/CommTuningHub.vue': '/acquisition/comm-tuning',
  'acquisition/TesterHub.vue': '/acquisition/tester',
  'ops/OpsHub.vue': '/ops',
  'data/DataPipelineHub.vue': '/data',
  'uplink/UplinkHub.vue': '/uplink',
}

const VIEWS_DIR = resolve(__dirname, '../../../views')

/** 解析 Hub 源文件里的页签定义 */
function parseHubTabs(): HubTab[] {
  const tabs: HubTab[] = []
  for (const [file, route] of Object.entries(HUB_ROUTES)) {
    const src = readFileSync(resolve(VIEWS_DIR, file), 'utf8')
    // 形如 { name: 'devices', label: '设备清单', ... }
    const re = /\{\s*name:\s*'([a-z0-9-]+)'\s*,\s*label:\s*'([^']+)'/g
    let m: RegExpExecArray | null
    while ((m = re.exec(src)) !== null) {
      tabs.push({ route, name: m[1]!, label: m[2]! })
    }
  }
  return tabs
}

const hubTabs = parseHubTabs()

describe('侧栏搜索索引', () => {
  it('解析到 Hub 页签定义（防止正则失效导致本测试假通过）', () => {
    // W1 下线 4 个页签（用户与角色 / 数据插值 / 基线对比 / 波形观测）后为 76；
    // W2-#13 上行三页合并为两页（上行通道 + 上行通道参数 + 上行服务状态 → 上行配置 / 上行状态）再减 1；
    // 第一阶段 A5/A6/A7 三个独立页并入宿主页（帧解析→报文编辑 / 异常码速查→连接测试 / 资产台账→设备详情）再减 3
    // B5 参数同步/限流并入宿主页再减 2
    // 第三阶段 C1 三个协议 Hub 合并为「协议调试台」：页签 4+3+4=11 全部保留，数量不变
    // 第 1 期冗余收口：16 个 Hub 壳页合并为 10 个（页签全部保留，仅 system 的日志与运维分组归并），数量不变
    expect(hubTabs.length).toBeGreaterThanOrEqual(70)
  })

  it('每个 Hub 页签都能按页签文字搜到', () => {
    const unreachable: string[] = []
    for (const tab of hubTabs) {
      const withTab = `${tab.route}?tab=${tab.name}`
      const label = tab.label.toLowerCase()
      // 命中条件：存在一条索引项指向该页签（或该 Hub 的裸路径），
      // 且该项的 title/alias/subtitle 包含页签文字
      const hit = navIndexItems.some((cmd) => {
        if (cmd.target !== withTab && cmd.target !== tab.route) return false
        const keys = [cmd.title, cmd.alias, cmd.subtitle]
          .filter((s): s is string => Boolean(s))
          .map((s) => s.toLowerCase())
        return keys.some((k) => k.includes(label) || label.includes(k))
      })
      if (!hit) unreachable.push(`${tab.route}?tab=${tab.name} 「${tab.label}」`)
    }
    expect(unreachable, `以下页签在侧栏搜索里搜不到：\n${unreachable.join('\n')}`).toEqual([])
  })

  it('索引项 id 唯一', () => {
    const ids = navIndexItems.map((c) => c.id)
    expect(new Set(ids).size).toBe(ids.length)
  })

  it('索引项都有 target', () => {
    for (const cmd of navIndexItems) {
      expect(cmd.target, `索引项 ${cmd.id} 缺少 target`).toBeTruthy()
    }
  })

  it('索引别名不与其它索引项的主标题冲突（避免搜同一词命中多个不同入口）', () => {
    const titles = new Set(navIndexItems.map((c) => c.title))
    for (const cmd of navIndexItems) {
      if (!cmd.alias) continue
      expect(titles.has(cmd.alias), `别名「${cmd.alias}」（${cmd.id}）与某条索引项主标题重复`).toBe(false)
    }
  })
})
