import { computed, ref } from 'vue'

/**
 * 实时数据页的视口高度分配（表格 / 图表）。
 *
 * N8: 原实现只按 `innerHeight-180` 给表格定高、图表再写死 360px，1366×768 下
 * 两卡合计远高于视口 → 页面外层滚动 + 表格内层滚动形成「双层滚动」，
 * 而真正要看的实时值反而被滚出视野。这里把固定开销逐项相加，剩余高度在
 * 表格与图表之间按比例分配，保证整页落在首屏内（只保留表格内滚动）。
 */
export function useRealtimeViewport() {
  // 固定开销逐项相加：顶栏 60 + 主区内边距 40 + 两张卡的头 65/61 + 两张卡的 body 内边距 80
  // + 卡间距 14 + 两张卡的上下边框各 1px（4）。此前按 270 估算，把两个卡头与 body 内边距
  // 少算了一半，1366×768 下两卡合计仍超出首屏约 189px；补上边框后按此口径分配，实测溢出 0。
  const CHROME_HEIGHT = 60 + 40 + 65 + 61 + 80 + 14 + 4

  /** 本机首次会话顶部还有一条安全横幅（Layout 渲染，40px + 12px 外边距）。
   *  它不是表格/图表，却真实占掉首屏高度，不扣掉的话两个卡会被顶出视口。 */
  function bannerOffset(): number {
    if (typeof document === 'undefined') return 0
    const el = document.querySelector('.security-notice')
    return el ? Math.round(el.getBoundingClientRect().height) + 12 : 0
  }

  const viewportH = ref(typeof window === 'undefined' ? 900 : window.innerHeight - bannerOffset())

  /** 校准视口高度：挂载后（DOM 已有安全横幅）与窗口 resize 时都要重算 */
  function refreshViewport() {
    if (typeof window === 'undefined') return
    viewportH.value = window.innerHeight - bannerOffset()
  }

  const tableHeight = computed(() => {
    const usable = viewportH.value - CHROME_HEIGHT
    return Math.max(220, Math.min(720, Math.round(usable * 0.55)))
  })

  const chartHeight = computed(() => {
    const usable = viewportH.value - CHROME_HEIGHT
    // 下限 170：留 180 时，首次会话有安全横幅的场景下可用高度只剩 396px，
    // 表格(218)+柱状图被顶到下限 180，两卡合计反而超出视口 8px。
    return Math.max(170, Math.min(480, usable - tableHeight.value))
  })

  return { tableHeight, chartHeight, refreshViewport }
}
