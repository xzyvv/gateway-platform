/**
 * ECharts 核心实例与组件注册（所有图表工具模块的共同底座）。
 *
 * 核心图表组件：Line + Grid / Tooltip / Legend + CanvasRenderer。
 *
 * 体积较大但使用频次低的组件（Graph 力导向图、Custom 自定义绘制、Bar 柱状图、
 * DataZoom 区域缩放、Title 标题、MarkLine 标线）拆到 `echarts-extra.ts`，
 * 只有用到的页面才加载，避免折线图页面也拖入整套拓扑图引擎
 * （GraphChart 单组件 ~150KB raw）。
 *
 * 注意：echarts.use 注册到全局单例，echarts-extra 复用同一个实例注册额外组件。
 *
 * 本模块只做「注册 + 导出实例」，不含任何业务逻辑；其余图表能力按职责分布在：
 *   - `echartsThemes.ts`   主题注册（gw-dark / gw-light）与暗色判定
 *   - `echartsRegistry.ts` 实例注册表、销毁、尺寸自适应、主题重绘
 *   - `echartsInit.ts`     建图、无障碍、减少动效、实例与宿主一致性
 *   - `echartsAppend.ts`   appendData 增量刷新与 setOption 节流
 */
import * as echarts from 'echarts/core'
import { LineChart } from 'echarts/charts'
import {
  GridComponent,
  TooltipComponent,
  LegendComponent,
} from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'

echarts.use([
  LineChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  CanvasRenderer,
])

export { echarts }
export default echarts
