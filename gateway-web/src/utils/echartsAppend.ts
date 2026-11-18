/**
 * 图表刷新节流窗口。
 *
 * 实时推送页面（Realtime / Monitor sparkline / Oscilloscope）在高频消息下
 * 每条都 setOption 全量重绘——30fps 推送时主线程被 setOption 占满，图表抖动、
 * 表格掉帧。把 N 次重绘合并到一个 ~10fps 的节流窗口里即可缓解 CPU 压力。
 *
 * 说明：本文件原先还提供基于 ECharts `appendData` 的增量刷新器
 * （createAppendDataUpdater / createThrottledSetter）。该方案要求 series 使用
 * 数组形式 data，而本项目图表一律用内联 data 数组——appendData 对内联数组
 * 必抛 TypeError，两函数因此从未被任何页面引用，已作为死代码移除。
 * 现网刷新路径统一为「setOption + 节流」，见 useRealtimeChart.ts。
 */

/** 默认节流 100ms —— ~10fps 的视觉刷新率，既不会肉眼察觉延迟，又把 N 次 setOption 合并成 1 次 */
export const APPEND_THROTTLE_MS = 100
