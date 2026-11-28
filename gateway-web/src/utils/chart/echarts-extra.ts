/**
 * ECharts 扩展组件注册（按需加载）。
 *
 * 这些组件体积大或使用频次低，从核心 `echarts.ts` 拆出，只有用到的页面
 * 才需要 import 本模块，从而减小普通折线/柱状图页面的 chunk 体积。
 *
 * 复用 `echarts/core` 的全局单例：echarts.use 注册到同一个实例，
 * 因此核心组件与扩展组件可在同一图表中混用。
 *
 * 包含组件：
 *   - GraphChart   力导向图（设备拓扑 DeviceTopology）
 *   - CustomChart  自定义系列绘制（甘特图 GanttChart）
 *   - DataZoomComponent  区域缩放（历史曲线 historyChart）
 *   - TitleComponent     图表标题
 *   - MarkLineComponent  标线（阈值/平均线，DiskHealthChart）
 */
import * as echarts from 'echarts/core'
import { GraphChart, CustomChart, BarChart } from 'echarts/charts'
import {
  DataZoomComponent,
  TitleComponent,
  MarkLineComponent,
} from 'echarts/components'

echarts.use([
  GraphChart,
  CustomChart,
  BarChart,
  DataZoomComponent,
  TitleComponent,
  MarkLineComponent,
])

// 复用核心模块的 echarts 单例，方便页面从同一入口取到带扩展注册的实例
export { default } from './echarts'
// 透传核心模块的所有辅助函数（initChart / disposeChart / applyEchartsTheme 等），
// 这样页面只需从 echarts-extra 一个入口导入即可同时拿到扩展注册与工具函数。
export * from './echarts'
