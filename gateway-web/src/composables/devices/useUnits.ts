/**
 * 常用工程单位清单（点位 / 虚拟点位 / 设备模板共用）
 *
 * 单位是上行数据的量纲标识，同义写法（℃ / C、kWh / KWH、m3 / m³）会让上层分组、
 * 统计出现重复维度。控件只允许从本清单选择，杜绝同义写法。
 *
 * 清单需覆盖仓库内置模板（templates/builtin、templates/custom）里实际出现的单位
 * （W / MW / MVar / kvar / m3 / m3/h / yuan …），否则用模板建出来的点位
 * 在单位下拉里没有对应项，显示成空白。
 *
 * 取值集的唯一来源是后端（`GET /api/point-fields`，读 `config::unit::RECOMMENDED_UNITS`），
 * 这里的常量只是离线 / 旧后端时的兜底，登录后由 `applyUnits` 原地替换。
 *
 * 注意：本清单同时决定 CT/PT 变比的适用范围（见 `useRatio.ts`）：
 * mV/kV/V → 只吃 PT，mA/kA/A → 只吃 CT，PF/cosφ → 两者都不吃，其余按 CT × PT。
 */

import { reactive } from 'vue'

/** 兜底单位清单（与后端 `RECOMMENDED_UNITS` 保持一致） */
export const unitOptions: string[] = reactive<string[]>([
  // 电压
  'V', 'kV', 'mV',
  // 电流
  'A', 'mA', 'kA',
  // 有功 / 无功 / 视在功率
  'W', 'kW', 'MW', 'var', 'kvar', 'MVar', 'VA', 'kVA',
  // 电能
  'Wh', 'kWh', 'MWh', 'kvarh',
  // 功率因数（无量纲，变比不参与）
  'PF', 'cosφ',
  // 频率 / 温度
  'Hz', '℃',
  // 流量 / 体积
  'm3', 'm3/h', 'L', 'L/h',
  // 压力
  'Pa', 'kPa', 'MPa', 'bar',
  // 速度 / 转速 / 比例
  'm/s', 'rpm', '%',
  // 时间
  's', 'min', 'h',
  // 其它
  'yuan',
])

/**
 * 用后端字典替换单位清单；空列表时保留兜底。
 *
 * 不做白名单裁剪：存量配置里的历史写法（kVar / m³）仍需能被选中回填，
 * 「是否规范」只由配置体检告警，不在下拉层阻断。
 */
export function applyUnits(values: string[]): void {
  const clean = values.map((v) => String(v).trim()).filter(Boolean)
  if (clean.length === 0) return
  unitOptions.splice(0, unitOptions.length, ...clean)
}

/**
 * 下拉选项 = 清单 + 当前值。
 *
 * 存量配置里可能有清单外的单位（早期版本允许自由输入），纯下拉若只给清单，
 * 老点位在下拉里找不到原值、清空后就再也选不回来。这里把当前值补进选项，
 * 保证「看得见、改得动、不会被静默改写」，但不新增可自由输入的入口。
 */
export function unitChoices(current?: string | null): string[] {
  const cur = (current ?? '').trim()
  return cur && !unitOptions.includes(cur) ? [...unitOptions, cur] : unitOptions
}
