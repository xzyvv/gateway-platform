/**
 * 交付自检入口：编排六项判定并汇总结果。
 *
 * 职责已按模块拆开，本文件只做「并行取数 → 逐项判定 → 汇总」：
 *   - selfCheckEvals.ts    六项判定（纯函数）与结论类型
 *   - deliveryChecklist.ts 交付检查单组装与 Markdown 导出
 *
 * 公共 API 面保持不变：既有页面（DeliveryCheck.vue）与测试仍从
 * `@/views/ops/delivery/selfCheck` 导入，无需改动调用点。
 */
import { getConfig } from '@/api/uplink/config'
import type { GatewayConfig } from '@/api/types'
import { getHealthReport } from '@/api/system/status'
import { listProtocolCapabilities } from '@/api/acquisition/protocols'
import { getNorthStatus } from '@/api/uplink/uplink'
import { getNtpStatus } from '@/api/system/network'
import { listCertificates } from '@/api/system/certificates'
import { listServices } from '@/api/system/watchdog'
import {
  evalBuses,
  evalCertificates,
  evalClock,
  evalNorth,
  evalProtocols,
  evalWatchdog,
  summarizeLevel,
  type CheckItem,
  type SelfCheckResult,
} from './selfCheckEvals'
import { extractErrorMessage } from '@/utils/error/errorHandler'

export {
  LEVEL_LABEL,
  LEVEL_TAG_TYPE,
  evalBuses,
  evalCertificates,
  evalClock,
  evalNorth,
  evalProtocols,
  evalWatchdog,
  summarizeLevel,
} from './selfCheckEvals'
export type { CheckItem, CheckLevel, SelfCheckResult } from './selfCheckEvals'

export {
  MANUAL_ITEMS,
  buildChecklist,
  buildSelfCheckMarkdown,
  checklistToMarkdown,
} from './deliveryChecklist'
export type {
  ChecklistEntry,
  ChecklistStatus,
  DeliveryChecklist,
  ManualItemDef,
} from './deliveryChecklist'

function failed(title: string, id: string, reason: string): CheckItem {
  return { id, title, level: 'fail', detail: `接口调用失败：${reason}` }
}

function rejectionReason(result: PromiseSettledResult<unknown>, fallback: string): string {
  if (result.status !== 'rejected') return fallback
  return extractErrorMessage(result.reason)
}

/* ============================== 采集与汇总 ============================== */

/**
 * 执行六项自检。
 *
 * 全部并行请求，单项接口失败不会中断其它项：失败项直接记为「未通过」并写明原因。
 * 复用已有只读端点（/api/protocols、/api/config、/api/health/report、/api/north/status、
 * /api/ntp/status、/api/certificates、/api/watchdog/services），不新增后端聚合 API。
 */
export async function runSelfCheck(): Promise<SelfCheckResult> {
  const [caps, config, report, north, ntp, certificates, services] = await Promise.allSettled([
    listProtocolCapabilities(),
    getConfig(),
    getHealthReport(),
    getNorthStatus(),
    getNtpStatus(),
    listCertificates(),
    listServices(),
  ])

  const items: CheckItem[] = []
  items.push(
    caps.status === 'fulfilled' && config.status === 'fulfilled'
      ? evalProtocols(caps.value, config.value)
      : failed('协议', 'protocol', rejectionReason(caps, rejectionReason(config, '配置接口不可用'))),
  )
  items.push(
    report.status === 'fulfilled' && config.status === 'fulfilled'
      ? evalBuses(report.value, config.value)
      : failed('总线', 'bus', rejectionReason(report, rejectionReason(config, '配置接口不可用'))),
  )
  items.push(north.status === 'fulfilled' ? evalNorth(north.value) : failed('北向', 'north', north.reason))
  items.push(ntp.status === 'fulfilled' ? evalClock(ntp.value) : failed('时钟', 'clock', ntp.reason))
  items.push(
    certificates.status === 'fulfilled'
      ? evalCertificates(certificates.value)
      : failed('证书', 'certificate', certificates.reason),
  )
  items.push(
    services.status === 'fulfilled'
      ? evalWatchdog(services.value)
      : failed('看门狗', 'watchdog', services.reason),
  )

  return {
    generatedAt: Date.now(),
    items,
    level: summarizeLevel(items.map((i) => i.level)),
  }
}

/** 采集需要的配置类型（供外部按同一口径构造夹具） */
export type { GatewayConfig }
