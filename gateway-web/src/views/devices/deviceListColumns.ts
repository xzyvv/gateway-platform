/**
 * 设备列表的表格列定义。
 *
 * 从 `DeviceList.vue` 拆出：列定义本身有 10 列、每列一个渲染函数，是本文件
 * 里最占篇幅也最独立的一块——它只依赖「行的哪些派生字段」和「几个动作回调」，
 * 与页面状态管理（加载、分页、搜索、路由跳转）无关。
 *
 * 所有依赖通过 `DeviceColumnContext` 显式传入，避免列定义反向依赖页面实例，
 * 也让这些渲染逻辑可以脱离页面单独单测。
 */
import { h, type VNodeChild } from 'vue'
import { ElButton, ElTag } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn, protocolColumn, addressColumn } from '@/composables/core/commonColumns'
import InlineEditCell from '@/components/InlineEditCell.vue'
import FreshnessBadge from '@/components/FreshnessBadge.vue'
import { formatDateTime } from '@/utils/datetime'
import type { DeviceConfig } from '@/api/types'
import type { DeviceEvidence } from '@/utils/deviceEvidence'
import type { TagType } from '@/utils/statusType'

/** 设备行：DeviceConfig 之外，列表接口可能带分组/标签等扩展字段；
 *  last_good_ts 由前端从退避接口聚合后注入，供「最后成功采集」列展示与排序。 */
export type DeviceRow = DeviceConfig & {
  group?: string | null
  tags?: string[] | null
  last_good_ts?: number
}

/** 通信质量兜底（后端真实采集质量）的展示值 */
export interface BackoffQuality {
  level: 'good' | 'warn' | 'bad'
  text: string
  intervalLabel: string
}

/** 设备在线态：由后端真实采集数据推导，而非设备配置里的开关字段 */
export type DeviceOnlineState = 'online' | 'offline' | 'unknown'

/**
 * 列定义所需的全部外部依赖。
 *
 * 显式声明而非闭包捕获页面实例：列定义是被页面创建的一次性对象，
 * 用参数传入可以让「渲染一列需要什么」一目了然。
 *
 * 数据类字段声明为 getter 语义：调用方传 `get x() { return ref.value }`，
 * 列渲染时才求值——若直接传 `.value` 快照，数据刷新后列会停留在旧值。
 */
export interface DeviceColumnContext {
  /** 本会话 UI 读写聚合的通信统计（按设备 ID） */
  readonly commsStats: Record<string, { avgLatency?: number | null } | undefined>
  /** 通信质量标签类型（由页面侧聚合逻辑给出） */
  commsQualityType: (deviceId: string) => TagType
  /** 通信质量标签文案 */
  commsQualityText: (deviceId: string) => string
  /** 后端真实采集质量兜底（无 UI 读写记录时用） */
  readonly backoffQuality: Record<string, BackoffQuality | undefined>
  /** 设备维度证据（最后成功时间 + 连续失败数 + 最近原始帧 hex） */
  readonly evidenceByDevice: Record<string, DeviceEvidence | undefined>
  /** 在线态判定结果 */
  readonly onlineStateByDevice: Record<string, DeviceOnlineState | undefined>
  /** 行内改名等字段级保存（返回是否保存成功） */
  saveDeviceField: (row: DeviceRow, patch: Partial<Pick<DeviceConfig, 'name'>>) => Promise<boolean>
  /** 打开某设备的资产表单 */
  openAsset: (row: DeviceRow) => void
  /** 编辑设备 */
  editDevice: (row: DeviceRow) => void
  /** 删除设备 */
  removeDevice: (row: DeviceRow) => void
  /** 跳转该设备的点位清单 */
  gotoPoints: (row: DeviceRow) => void
  /** 是否具备写权限（device.write） */
  readonly canWriteDevice: boolean
}

/**
 * 构建设备表的列定义数组。
 *
 * 列顺序即页面呈现顺序：状态 / 通信质量 / 最后成功采集 / 诊断证据 →
 * 设备 ID / 名称 / 模板 / 总线 / 协议 / 地址 / 分组 / 标签 → 操作。
 */
export function createDeviceColumns(ctx: DeviceColumnContext): ColumnDef<DeviceRow>[] {
  return [
    {
      key: 'online',
      title: '状态',
      width: 76,
      cell: (row) => {
        const state = ctx.onlineStateByDevice[row.id] ?? 'unknown'
        const label = state === 'online' ? '在线' : state === 'offline' ? '离线' : '未知'
        // 未知态要显式呈现：设备无点位/无采集记录时，不能默认给个绿灯
        return h('div', { class: 'device-state' }, [
          h('span', { class: ['state-dot', state] }),
          h('span', { class: 'state-label' }, label),
        ])
      },
    },
    {
      key: 'comms',
      title: '通信质量',
      width: 112,
      cell: (row) => {
        const stat = ctx.commsStats[row.id]
        if (stat) {
          // avgLatency 可能为 NaN / null（后端字段缺失或除零），直接 toFixed 会渲染出
          // "NaNms" 甚至抛 TypeError 导致整页错误边界
          const latency = Number(stat.avgLatency)
          return h('div', { class: 'comms-cell' }, [
            h(ElTag, { size: 'small', type: ctx.commsQualityType(row.id), effect: 'dark', round: true }, () =>
              ctx.commsQualityText(row.id),
            ),
            h('span', { class: 'comms-latency' }, Number.isFinite(latency) ? `${latency.toFixed(0)}ms` : '—'),
          ])
        }
        // 本会话无 UI 读写记录 → 用后端采集退避的真实质量兜底
        const bq = ctx.backoffQuality[row.id]
        if (bq) {
          const type = bq.level === 'good' ? 'success' : bq.level === 'warn' ? 'warning' : 'danger'
          return h('div', { class: 'comms-cell' }, [
            h(ElTag, { size: 'small', type, effect: 'dark', round: true }, () => bq.text),
            h('span', { class: 'comms-latency' }, bq.intervalLabel),
          ])
        }
        return h('span', { class: 'text-placeholder' }, '暂无数据')
      },
    },
    {
      key: 'last_good_ts',
      title: '最后成功采集',
      width: 124,
      sortable: true,
      // 用 FreshnessBadge 展示相对时间（「3 分钟前」），悬停显示绝对时间；
      // 无数据（从未成功采集）显示「从未」，让「设备坏了」与「值不变」一目了然。
      cell: (row) => {
        const ts = row.last_good_ts
        if (!ts) return h('span', { class: 'text-placeholder' }, '从未')
        return h(FreshnessBadge, { timestamp: ts })
      },
    },
    {
      key: 'evidence',
      title: '诊断证据',
      // 13 列横向滚动时这列最吃宽：收到 150px，内容仍保留「连续失败」标签 + hex 截断，
      // 完整报文与失败原因走 title 悬停
      width: 150,
      // 设备维度证据：让「为什么离线」一屏可答。
      // 展示最近一帧原始报文（hex）+ 失败原因；hover 出完整 hex 与时间。
      cell: (row) => {
        const ev = ctx.evidenceByDevice[row.id]
        if (!ev || (!ev.lastFrameHex && !ev.consecutiveFailures)) {
          return h('span', { class: 'text-placeholder' }, '—')
        }
        const children: VNodeChild[] = []
        // 连续失败次数：>0 时是「设备真的在失败」的硬证据
        if (ev.consecutiveFailures > 0) {
          children.push(
            h(ElTag, { size: 'small', type: 'danger', effect: 'plain', round: true, class: 'ev-fail-tag' },
              () => `连续失败 ${ev.consecutiveFailures}`),
          )
        }
        // 最近原始帧：hex 截断显示，完整内容放 title
        if (ev.lastFrameHex) {
          const shortHex = ev.lastFrameHex.length > 24
            ? `${ev.lastFrameHex.slice(0, 24)}…`
            : ev.lastFrameHex
          children.push(
            h('span', {
              class: ['ev-hex', ev.lastFrameFailed ? 'is-failed' : ''],
              title: `${ev.lastFrameDirection === 'request' ? '请求' : '响应'}帧 @ ${formatDateTime(ev.lastFrameTs)}\n${ev.lastFrameHex}${ev.lastFrameError ? `\n失败原因: ${ev.lastFrameError}` : ''}`,
            }, shortHex),
          )
        }
        // 失败原因：异常码/CRC 错等，是「设备没坏但数据不对」的关键线索
        if (ev.lastFrameError) {
          children.push(
            h(ElTag, { size: 'small', type: 'warning', effect: 'plain', round: true, class: 'ev-err-tag' },
              () => ev.lastFrameError),
          )
        }
        return h('div', { class: 'ev-cell' }, children)
      },
    },
    { key: 'id', title: '设备 ID', width: 128, ellipsis: true, mono: true, sortable: true },
    {
      key: 'name',
      title: '设备名称',
      width: 128,
      sortable: true,
      // 行内改名：改个错别字不必再开弹窗、重新校验整套表单
      cell: (row) =>
        h(InlineEditCell, {
          value: row.name,
          label: '设备名称',
          maxlength: 64,
          save: (v: string | number) => ctx.saveDeviceField(row, { name: String(v) }),
        }),
    },
    { key: 'template', title: '模板', minWidth: 150, ellipsis: true, sortable: true },
    { key: 'bus', title: '总线', width: 90, sortable: true },
    protocolColumn<DeviceRow>({ width: 100, sortable: true }),
    addressColumn<DeviceRow>({ title: '设备地址', width: 120, sortable: true }),
    { key: 'group', title: '分组', width: 110, ellipsis: true, sortable: true, cell: (row) => row.group || '—' },
    {
      key: 'tags',
      title: '标签',
      minWidth: 120,
      ellipsis: true,
      sortable: true,
      cell: (row) => {
        const tags = row.tags ?? []
        if (!tags.length) return h('span', { class: 'text-placeholder' }, '—')
        const nodes = tags
          .slice(0, 3)
          .map((t) => h(ElTag, { key: t, size: 'small', type: 'info', effect: 'plain', round: true, class: 'dev-tag' }, () => t))
        if (tags.length > 3) {
          nodes.push(h(ElTag, { size: 'small', type: 'info', effect: 'plain', round: true }, () => `+${tags.length - 3}`))
        }
        return h('span', null, nodes)
      },
    },
    actionColumn<DeviceRow>({
      // 固定在右缘：横向滚动时编辑/删除不会被滚出可视区
      // 150 → 200：并入「资产」入口后写权限用户有 4 个链接按钮，原宽度会挤压换行
      width: 200,
      fixed: 'right',
      // 按钮级权限：编辑/删除仅 device.write 可见
      cell: (row) => {
        const children = [
          // E9：设备名不可点，用户想看点位还得自己先找到点位页再选设备。
          // 这里直接带参数跳到该设备的点位清单（PointList 支持 ?device= 预选）。
          h(ElButton, {
            size: 'small',
            type: 'primary',
            link: true,
            onClick: () => ctx.gotoPoints(row),
          }, () => '点位'),
          // A7：资产信息按设备维度打开，不再单独占一个页签
          h(ElButton, {
            size: 'small',
            type: 'primary',
            link: true,
            onClick: () => ctx.openAsset(row),
          }, () => '资产'),
        ]
        if (ctx.canWriteDevice) {
          children.push(
            h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => ctx.editDevice(row) }, () => '编辑'),
            h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => ctx.removeDevice(row) }, () => '删除'),
          )
        }
        return h('span', null, children)
      },
    }),
  ]
}
