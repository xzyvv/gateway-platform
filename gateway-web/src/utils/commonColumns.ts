/**
 * 跨页面通用列定义（DataTable 的 ColumnDef 工厂）
 *
 * 背景：`ColumnDef` 是纯数据，同一类列（时间 / 协议 / 地址 / 状态标签）在
 * 十几个页面里各写一遍，改列名或改列宽要挨个文件找。收口到本模块后：
 * - 改「时间」列的格式或宽度 → 只改这里；
 * - 各页面差异（宽度 / 排序 / 自定义取色）通过参数覆盖，不复制整段 `h()`。
 *
 * 约束：这里只做「展示形态」的收敛，不掺业务判断——颜色映射仍走
 * `utils/statusType.ts`，协议元数据仍走 `composables/devices/useProtocol.ts`。
 */
import { h, type VNodeChild } from 'vue'
import { ElTag } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { formatDateTime } from '@/utils/datetime'
import { protocolLabel, protocolType } from '@/composables/devices/useProtocol'
import type { TagType } from '@/utils/statusType'

/** 行类型只要求是对象；取值统一走 key 下标，避免各页接口无索引签名导致泛型约束报错 */
type AnyRow = Record<string, unknown>

function readValue<T extends object>(row: T, key: string): unknown {
  return (row as AnyRow)[key]
}

export interface TimeColumnOptions<T> {
  title?: string
  width?: number
  sortable?: boolean
  fixed?: boolean | 'left' | 'right'
  /** 自定义格式化；默认 `formatDateTime`（24 小时制完整日期时间） */
  format?: (value: unknown, row: T) => string
}

/**
 * 时间列。
 *
 * 默认宽度 170（`2026/10/24 12:31:14` 的稳定宽度），默认 24 小时制完整时间。
 * 只需时分秒的报文类列表用 `format: (v) => formatTimeOfDay(v)`；
 * 审计类另有格式化函数时用 `format` 覆盖。
 */
export function timeColumn<T extends object = AnyRow>(
  key: string,
  options: TimeColumnOptions<T> = {},
): ColumnDef<T> {
  const { title = '时间', width = 170, sortable, fixed, format } = options
  return {
    key,
    title,
    width,
    fixed,
    sortable,
    cell: (row) => {
      const value = readValue(row, key)
      return format
        ? format(value, row)
        : formatDateTime(value as number | string | Date | null | undefined)
    },
  }
}

export interface ProtocolColumnOptions {
  title?: string
  width?: number
  sortable?: boolean
  /** 是否把协议名翻成中文标签（默认 true）；扫描结果等保留原始协议名时传 false */
  useLabel?: boolean
  /** 覆盖默认协议标签表（极少用，便于特殊页面复用） */
  labels?: Record<string, string>
}

/**
 * 协议列：统一 `el-tag` + 后端注册表下发的颜色 + 中文标签。
 *
 * 过去各页面要么只显示协议名、要么自己写一份 `protocolTagType()`，同一协议
 * 在不同页面颜色和文案都可能不同。这里固定为「暗色圆角小标签」。
 */
export function protocolColumn<T extends object = AnyRow>(
  options: ProtocolColumnOptions & { key?: string } = {},
): ColumnDef<T> {
  const { key = 'protocol', title = '协议', width = 100, sortable, useLabel = true, labels } = options
  const labelTable = labels ?? protocolLabel
  return {
    key,
    title,
    width,
    sortable,
    cell: (row) => {
      const protocol = String(readValue(row, key) ?? '')
      const text = useLabel ? labelTable[protocol] || protocol : protocol
      return h(ElTag, { type: protocolType(protocol), size: 'small', effect: 'dark', round: true }, () => text)
    },
  }
}

export interface AddressColumnOptions {
  title?: string
  width?: number
  minWidth?: number
  align?: 'left' | 'center' | 'right'
  /** 等宽字体 + 不折行，便于逐位比对地址（默认 true） */
  mono?: boolean
  ellipsis?: boolean
  sortable?: boolean
}

/**
 * 地址列：等宽、右对齐、超长省略。
 *
 * 地址（寄存器地址 / 设备地址 / 从站地址）需要逐位比对，等宽 + 右对齐是
 * 现场看表的基本要求，不该每个页面各自记得加。
 */
export function addressColumn<T extends object = AnyRow>(
  options: AddressColumnOptions & { key?: string } = {},
): ColumnDef<T> {
  const {
    key = 'address',
    title = '地址',
    width = 100,
    minWidth,
    align = 'right',
    mono = true,
    ellipsis = true,
    sortable,
  } = options
  return { key, title, width, minWidth, align, mono, ellipsis, sortable }
}

export interface TagColumnOptions<T> {
  title: string
  width?: number
  sortable?: boolean
  /** 行 → 标签颜色 */
  type: (row: T) => TagType
  /** 行 → 标签文案；不传则直接显示字段原值 */
  label?: (row: T) => string
  effect?: 'dark' | 'light' | 'plain'
  round?: boolean
}

export interface ActionColumnOptions<T> {
  /** 默认 `actions`；只读占位列（如「操作」恒显示 `-`）可另行指定，避免与真实操作列同 key */
  key?: string
  title?: string
  width?: number
  minWidth?: number
  align?: 'left' | 'center' | 'right'
  /** 默认不固定；长表需要横向滚动时传 `'right'`，让操作入口始终在手边 */
  fixed?: boolean | 'left' | 'right'
  /** 行 → 操作按钮。返回单个 VNode 或数组均可（数组按下标顺序平铺渲染） */
  cell: (row: T) => VNodeChild
}

/**
 * 「操作」列。
 *
 * 31 个页面各自声明了 `{ key: 'actions', title: '操作', width, cell }`，
 * 想把「操作」改叫「管理」或统一收敛宽度就得挨个文件改。收口到这里后各页只保留
 * **差异部分**（宽度 / 是否固定 / 按钮实现），列名与 key 统一由此处决定。
 *
 * 刻意不在这里包一层容器：各页的操作单元格既有单按钮、也有 `h('span', [...])`
 * 多按钮，还带各自的 `.row-actions` 类做间距。强行套一层会打乱现有 scoped 样式。
 */
export function actionColumn<T extends object = AnyRow>(options: ActionColumnOptions<T>): ColumnDef<T> {
  const { key = 'actions', title = '操作', width = 120, minWidth, align, fixed, cell } = options
  return { key, title, width, minWidth, align, fixed, cell }
}

/**
 * 只读标签列（状态 / 结果 / 操作类型）。
 *
 * 只负责「同一列形态」：小号、暗色、圆角；颜色与文案由调用方通过
 * `type` / `label` 提供，避免把业务映射塞进公共模块。
 */
export function tagColumn<T extends object = AnyRow>(
  key: string,
  options: TagColumnOptions<T>,
): ColumnDef<T> {
  const { title, width = 90, sortable, type, label, effect = 'dark', round = true } = options
  return {
    key,
    title,
    width,
    sortable,
    cell: (row) =>
      h(
        ElTag,
        { size: 'small', type: type(row), effect, round },
        () => (label ? label(row) : String(readValue(row, key) ?? '')),
      ),
  }
}
