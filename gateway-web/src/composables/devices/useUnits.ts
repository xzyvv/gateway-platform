/**
 * 常用工程单位清单（点位 / 虚拟点位共用）
 *
 * 单位是上行数据的量纲标识，同义写法（℃ / C、kWh / KWH）会让上层分组、
 * 统计出现重复维度。需要扩展时只改这里，所有调用方自动同步。
 */
export const unitOptions: string[] = [
  '℃', 'kWh', 'MWh', 'kVarh', 'V', 'kV', 'A', 'kW', 'kVar', 'Hz',
  'Pa', 'MPa', 'bar', 'm³', 'L', 'm/s', 'rpm', '%', 's',
]
