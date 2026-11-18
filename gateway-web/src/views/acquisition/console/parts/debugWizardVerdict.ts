/**
 * 调试向导的汇总结论判定。
 *
 * 从 `DebugWizard.vue` 拆出：汇总结论是「交接班时唯一要看的结论」，
 * 它的判定顺序有明确优先级（严重 > 异常 > 未完成 > 已确认但检查未跑完 > 全部通过），
 * 集中成纯函数后可以脱离页面单测，避免结论文案被后续改动悄悄改错。
 */
import type { ProbeResult } from './debugWizardProbes'

/** 汇总结论的色调 */
export type VerdictTone = 'danger' | 'warning' | 'success' | 'info'

/** 汇总判定所需的输入 */
export interface VerdictInput {
  /** 已勾选完成的检查项数 */
  doneCount: number
  /** 当前流程的检查项总数 */
  stepTotal: number
  /** 当前流程里可自动检查的步骤总数 */
  probeTotal: number
  /** 已执行的自动检查结果 */
  probes: ProbeResult[]
}

/** 已执行的自动检查按结论分类 */
export interface ProbeStats {
  /** 已执行的自动检查数 */
  done: number
  danger: number
  warning: number
  success: number
}

/** 汇总卡里的下一步入口 */
export interface NextAction {
  label: string
  to: string
}

/**
 * 统计已执行的自动检查按结论分类。
 *
 * 严重问题必须单独计数：它不能被「进度 5/5」掩盖——
 * 进度满但有严重问题时，结论必须是「先处置严重问题」。
 */
export function computeProbeStats(probes: ProbeResult[]): ProbeStats {
  return {
    done: probes.length,
    danger: probes.filter((p) => p.tone === 'danger').length,
    warning: probes.filter((p) => p.tone === 'warning').length,
    success: probes.filter((p) => p.tone === 'success').length,
  }
}

/** 汇总结论的色调：严重 > 异常 > 未完成 > 已确认但自动检查未跑完 > 全部通过 */
export function computeVerdictTone(input: VerdictInput, stats: ProbeStats): VerdictTone {
  if (stats.danger) return 'danger'
  if (stats.warning) return 'warning'
  const remaining = input.stepTotal - input.doneCount
  if (remaining > 0) return 'info'
  if (stats.done < input.probeTotal) return 'info'
  return 'success'
}

/**
 * 汇总结论文案。
 *
 * 关键约束：勾选全完成 ≠ 自动检查全跑过。
 * 没跑自动检查就说「未发现异常」是假结论——现场会据此判断「设备没问题」而收工。
 */
export function computeVerdictText(input: VerdictInput, stats: ProbeStats): string {
  if (stats.danger) {
    return `自动检查发现 ${stats.danger} 项严重问题，请先按步骤内的提示处置后再继续`
  }
  if (stats.warning) {
    return `自动检查发现 ${stats.warning} 项异常，建议先处理再继续排查`
  }
  const remaining = input.stepTotal - input.doneCount
  if (remaining > 0) {
    return `还有 ${remaining} 项检查未完成，按顺序逐项确认即可定位问题`
  }
  if (stats.done < input.probeTotal) {
    return `检查项已全部确认，但还有 ${input.probeTotal - stats.done} 项自动检查未执行，结论仅供参考`
  }
  return '全部检查项已完成，自动检查未发现异常'
}

/**
 * 汇总卡里的下一步入口：按路由去重，避免同一页签重复出现。
 */
export function computeNextActions(
  steps: Array<{ action?: NextAction }>,
): NextAction[] {
  const seen = new Set<string>()
  const out: NextAction[] = []
  for (const s of steps) {
    if (!s.action || seen.has(s.action.to)) continue
    seen.add(s.action.to)
    out.push(s.action)
  }
  return out
}
