/**
 * 图表工具公共出口（聚合层）。
 *
 * 职责已按模块拆开，本文件只做「一处导入，拿到全部」：
 *   - `echartsSetup.ts`   echarts 实例与核心组件注册（Line/Grid/Tooltip/Legend/Canvas）
 *   - `echartsThemes.ts`  主题注册（gw-dark / gw-light）与暗色判定
 *   - `echartsRegistry.ts`实例注册表、销毁、尺寸自适应、主题重绘
 *   - `echartsInit.ts`    建图、无障碍、减少动效、实例与宿主一致性
 *   - `echartsAppend.ts`  appendData 增量刷新与 setOption 节流
 *
 * 公共 API 面保持不变：既有页面 / composable 全部继续从 `@/utils/echarts`
 * 导入，无需改动任何调用点。
 */

// 默认导出：echarts 核心单例（含 graphic / init 等）
export { default } from './echartsSetup'

// 主题与注册表
export {
  applyEchartsTheme,
  cssVar,
  disposeChart,
  registerChart,
  unregisterChart,
  __liveChartCountForTests,
  __resetEchartsThemeCacheForTests,
  __resetLiveChartsForTests,
} from './echartsRegistry'

// 建图
export { applyReducedMotion, ensureChart, initChart } from './echartsInit'

// 增量刷新
export {
  APPEND_THROTTLE_MS,
  createAppendDataUpdater,
  createThrottledSetter,
} from './echartsAppend'
export type { AppendDataTask, AppendDataUpdater } from './echartsAppend'
