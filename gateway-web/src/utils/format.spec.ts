import { describe, expect, it } from 'vitest'
import { formatBytes, formatDuration, formatElapsed, formatRelative } from './format'

/**
 * 契约：这三个函数是**全站唯一**的字节/时长格式化口径。
 *
 * 过去 6 个文件各写一遍，已经出现过「同一个文件 1.5 MB / 1.50 MB / 1536 KB」
 * 三种显示（配置版本 / 网关诊断 / 日志归档），现场对比截图时以为数据不一致。
 * 因此这里锁的不是「某个实现细节」，而是对外承诺的格式。
 */
describe('formatBytes', () => {
  it('非法与零值降级为 0 B，不输出 NaN / Infinity', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(-1)).toBe('0 B')
    expect(formatBytes(Number.NaN)).toBe('0 B')
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe('0 B')
  })

  it('B 取整、KB 起固定 1 位小数', () => {
    expect(formatBytes(1)).toBe('1 B')
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(1023)).toBe('1023 B')
    expect(formatBytes(1024)).toBe('1.0 KB')
    expect(formatBytes(1536)).toBe('1.5 KB')
  })

  it('按 1024 进位而不是 1000', () => {
    expect(formatBytes(1024 ** 2)).toBe('1.0 MB')
    expect(formatBytes(2 * 1024 ** 3)).toBe('2.0 GB')
    expect(formatBytes(1024 ** 4)).toBe('1.0 TB')
  })

  it('四舍五入顶到 1024 时进一位，不显示「1024.0 KB」', () => {
    // 1048575 B = 1023.999 KB → 若直接 toFixed(1) 会得到「1024.0 KB」
    expect(formatBytes(1024 ** 2 - 1)).toBe('1.0 MB')
    expect(formatBytes(1024 ** 3 - 1)).toBe('1.0 GB')
  })

  it('超过 TB 仍停在 TB（不引入 PB），避免为不可能的分支加单位', () => {
    expect(formatBytes(1024 ** 5)).toBe('1024.0 TB')
  })
})

describe('formatDuration', () => {
  it('缺失/非法时间返回占位符，而不是 0 秒', () => {
    expect(formatDuration(null)).toBe('-')
    expect(formatDuration(undefined)).toBe('-')
    expect(formatDuration(Number.NaN)).toBe('-')
  })

  it('不足 1 分钟不写「0 分」', () => {
    expect(formatDuration(0)).toBe('0 秒')
    expect(formatDuration(42_000)).toBe('42 秒')
  })

  it('分钟、小时、天三级截断，只保留有价值的单位', () => {
    expect(formatDuration(65_000)).toBe('1 分 5 秒')
    expect(formatDuration(3_600_000 + 5 * 60_000 + 8_000)).toBe('1 小时 5 分 8 秒')
    expect(formatDuration(2 * 86_400_000 + 3 * 3_600_000 + 4 * 60_000)).toBe('2 天 3 小时 4 分')
  })
})

describe('formatElapsed', () => {
  it('缺失/非法/负数返回占位符', () => {
    expect(formatElapsed(null)).toBe('-')
    expect(formatElapsed(Number.NaN)).toBe('-')
    expect(formatElapsed(-5)).toBe('-')
  })

  it('不足 1 秒用毫秒：现场排查串口超时看的是 ms', () => {
    expect(formatElapsed(0)).toBe('0 ms')
    expect(formatElapsed(850)).toBe('850 ms')
  })

  it('999.6 ms 四舍五入后显示 1 秒，而不是 1000 ms', () => {
    expect(formatElapsed(999.6)).toBe('1 秒')
  })

  it('1 秒以上保留至多 2 位小数并去掉尾零', () => {
    expect(formatElapsed(1234)).toBe('1.23 秒')
    expect(formatElapsed(1250)).toBe('1.25 秒')
    expect(formatElapsed(5000)).toBe('5 秒')
  })
})

describe('formatRelative', () => {
  it('非法与负数返回占位符，不输出 NaN 秒前', () => {
    expect(formatRelative(null)).toBe('-')
    expect(formatRelative(undefined)).toBe('-')
    expect(formatRelative(Number.NaN)).toBe('-')
    expect(formatRelative(Number.POSITIVE_INFINITY)).toBe('-')
    expect(formatRelative(-5)).toBe('-')
  })

  it('5 秒内显示「刚刚」，不显示「0 秒前」', () => {
    expect(formatRelative(0)).toBe('刚刚')
    expect(formatRelative(4)).toBe('刚刚')
    expect(formatRelative(5)).toBe('5 秒前')
  })

  it('单位之间带空格（与 formatDuration / formatElapsed 口径一致）', () => {
    expect(formatRelative(30)).toBe('30 秒前')
    expect(formatRelative(60)).toBe('1 分钟前')
    expect(formatRelative(3600)).toBe('1 小时前')
  })

  it('按 60 / 3600 边界截断，只保留有价值的单位', () => {
    expect(formatRelative(59)).toBe('59 秒前')
    expect(formatRelative(119)).toBe('1 分钟前')
    expect(formatRelative(3599)).toBe('59 分钟前')
    expect(formatRelative(7200)).toBe('2 小时前')
  })

  it('超过一天仍按小时计，不引入「N 天前」', () => {
    expect(formatRelative(25 * 3600)).toBe('25 小时前')
  })

  it('小数秒向下取整，不显示「30.5 秒前」', () => {
    expect(formatRelative(30.9)).toBe('30 秒前')
  })
})
