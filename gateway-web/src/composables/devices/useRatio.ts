/**
 * CT / PT 变比适用范围（按点位单位判定）
 *
 * 变比只对「一次侧 → 二次侧」有物理意义的量生效：
 * - 电压（V/kV/mV）：只受 PT 影响
 * - 电流（A/kA/mA）：只受 CT 影响
 * - 功率/电量（W/kW/var/kWh/kVarh…）：同时受 CT × PT 影响（P = U × I）
 * - 功率因数（PF/cosφ）：无量纲比值，两者都不生效
 * - 单位为空或不在上述清单：按 CT × PT 处理，与后端 `convert_value` 的兼容行为一致
 *
 * 后端 `decode.rs::convert_value` 使用同一套判定，改动时两边需同步。
 */
export type RatioScope = 'pt' | 'ct' | 'both' | 'none'

const PT_UNITS = new Set(['v', 'kv', 'mv'])
const CT_UNITS = new Set(['a', 'ka', 'ma'])
const PF_UNITS = new Set(['pf', 'cos', 'cosφ', 'cosphi', '功率因数'])

/** 单位 → 变比适用范围（大小写不敏感） */
export function ratioScopeOf(unit?: string | null): RatioScope {
  const u = (unit ?? '').trim().toLowerCase()
  if (PT_UNITS.has(u)) return 'pt'
  if (CT_UNITS.has(u)) return 'ct'
  if (PF_UNITS.has(u)) return 'none'
  return 'both'
}
