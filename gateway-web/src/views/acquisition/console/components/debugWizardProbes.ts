/**
 * 调试向导的「自动检查」实现。
 *
 * 从 `DebugWizard.vue` 拆出：三类探针（总线 / 设备 / 健康）都是纯函数式的
 * 判定逻辑——给定上下文得出一个结论对象，既不碰组件状态也不发请求，
 * 与页面（筛选、勾选进度、渲染）职责正交。
 *
 * 拆出来后这些判定可以脱离页面单独单测，也避免页面文件被探针细节淹没。
 */
import type { HealthProblem } from '@/api/status'
import type { BusConfig, DeviceConfig } from '@/api/types'
import { problemLevelMeta } from '@/utils/healthStatus'
import {
  PROTOCOL_LABELS,
  busMatchesProtocol,
  busProtocolName,
  type WizardProtocol,
} from './debugWizardFlows'

/** 单条检查项的自动检查结果 */
export interface ProbeResult {
  tone: 'success' | 'warning' | 'danger' | 'info'
  message: string
  problems?: HealthProblem[]
  goto?: { label: string; to: string }
}

/** 总线探针需要的上下文 */
export interface BusProbeContext {
  /** 全部总线配置 */
  buses: BusConfig[]
  /** 当前选中的调试总线（port_name） */
  selectedBus: string
  /** 当前排查路径的协议 */
  protocol: WizardProtocol
}

/** 设备探针需要的上下文 */
export interface DeviceProbeContext {
  /** 全部设备配置 */
  devices: DeviceConfig[]
  /** 当前选中的调试设备 ID */
  selectedDeviceId: string
}

function protocolText(name: string): string {
  return PROTOCOL_LABELS[name as WizardProtocol] ?? name
}

/**
 * 健康问题是否与当前排查协议相关：串口协议只看总线/采集类，MQTT 只看上行类。
 *
 * 过滤的理由：健康报告是全设备视角的，串口排查时把 MQTT 队列积压报出来只会
 * 让人去查错方向——现场按错误线索拆机柜的代价很高。
 */
export function isRelevantProblem(problem: HealthProblem, protocol: WizardProtocol): boolean {
  const text = `${problem.code} ${problem.message}`.toUpperCase()
  if (protocol === 'mqtt') {
    return /MQTT|UPLINK|NORTH|QUEUE|上行|转发|队列/.test(text)
  }
  if (protocol === 'generic') return true
  return /BUS|DEVICE|POINT|COLLECT|MODBUS|DLT|CJT|CRC|TIMEOUT|总线|设备|点位|采集|超时|校验/.test(text)
}

/**
 * 总线探针：判断当前调试总线是否与本次排查协议匹配。
 *
 * 三种「还不能下结论」的情况都给出可执行入口，而不是只说一句「不匹配」——
 * 现场需要知道下一步点哪里。
 */
export function runBusProbe(ctx: BusProbeContext): ProbeResult {
  if (!ctx.buses.length) {
    return {
      tone: 'warning',
      message: '未配置总线，请先到配置管理创建一条总线',
      goto: { label: '去配置管理', to: '/system/config?tab=wizard' },
    }
  }
  const bus = ctx.buses.find((b) => b.port_name === ctx.selectedBus)
  if (!bus) {
    return {
      tone: 'warning',
      message: '尚未选中调试总线，请先选择一条总线再排查',
      goto: { label: '去选择总线', to: '/acquisition/console?tab=packet-editor' },
    }
  }
  const actual = protocolText(busProtocolName(bus))
  if (!busMatchesProtocol(bus, ctx.protocol)) {
    return {
      tone: 'warning',
      message: `当前目标总线 ${bus.port_name}（${actual}）与本次排查协议 ${PROTOCOL_LABELS[ctx.protocol]} 不匹配，请先切换总线`,
      goto: { label: '去选择总线', to: '/acquisition/console?tab=packet-editor' },
    }
  }
  return {
    tone: 'success',
    message: `当前目标总线 ${bus.port_name}（${actual}）与本次排查协议匹配`,
  }
}

/**
 * 设备探针：确认当前调试设备已选中。
 *
 * 与总线探针同类：设备没选中的时候谈不上「设备有没有问题」，
 * 先提示去选设备。
 */
export function runDeviceProbe(ctx: DeviceProbeContext): ProbeResult {
  if (!ctx.devices.length) {
    return {
      tone: 'warning',
      message: '尚未配置任何设备，请先到设备管理添加设备',
      goto: { label: '去设备管理', to: '/devices?tab=devices' },
    }
  }
  const device = ctx.devices.find((d) => d.id === ctx.selectedDeviceId)
  if (!device) {
    return {
      tone: 'warning',
      message: `已加载 ${ctx.devices.length} 台设备，但当前调试设备未选中，请先选择设备`,
      goto: { label: '去选择设备', to: '/acquisition/console?tab=auto-detect' },
    }
  }
  return {
    tone: 'success',
    message: `共 ${ctx.devices.length} 台设备，当前调试设备：${device.name}（${device.id}）`,
  }
}

/**
 * 健康探针：按当前协议过滤健康报告的问题项。
 *
 * `next` 为本次刷新拿到的快照（null 表示读取失败），
 * 读取失败必须报 danger——不能静默说「未发现异常」，那是假结论。
 */
export function runHealthProbe(
  next: { problems?: HealthProblem[] } | null,
  fallback: { problems?: HealthProblem[] } | null | undefined,
  protocol: WizardProtocol,
  errorMessage: string | null | undefined,
): ProbeResult {
  if (errorMessage && !next) {
    return { tone: 'danger', message: `健康报告读取失败：${errorMessage}` }
  }
  const problems = (next?.problems ?? fallback?.problems ?? []).filter((p) =>
    isRelevantProblem(p, protocol),
  )
  return {
    tone: problems.length ? 'warning' : 'success',
    message: problems.length
      ? `发现 ${problems.length} 条与本次排查相关的异常`
      : '健康报告未发现相关异常',
    problems,
  }
}

/** 健康问题等级 → 标签配色（统一走 utils/healthStatus，与健康报告页同一份口径，DD-72） */
export function levelTagType(level: HealthProblem['level']): 'danger' | 'warning' | 'info' {
  const type = problemLevelMeta(level).type
  return type === 'danger' || type === 'warning' ? type : 'info'
}
