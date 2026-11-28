/**
 * 点位清单的列定义
 *
 * 从 PointList.vue 迁出：列定义有 140+ 行且只服务表格渲染，
 * 与页面的「状态与编排」职责分开后，页面文件才能降到 500 行预算以内。
 *
 * 列里要用到页面的动作（读值 / 编辑 / 删除 / 写入白名单 / 行内编辑），
 * 这里通过回调参数注入，不反向依赖页面组件，也不依赖任何响应式实例
 * （工厂在页面 setup 里调用一次即可）。
 */
import { h } from 'vue'
import { ElButton, ElDropdown, ElDropdownItem, ElDropdownMenu, ElIcon, ElSwitch, ElTag } from 'element-plus'
import { ArrowDown, Edit, Reading } from '@element-plus/icons-vue'
import InlineEditCell from '@/components/InlineEditCell.vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn, protocolColumn } from '@/composables/core/commonColumns'
import type { InlinePatch, PointRow } from '@/composables/devices/usePoints'

export interface PointColumnActions {
  /** 写权限（point.write）：决定编辑 / 删除入口是否露出 */
  canWrite: () => boolean
  /** 读取当前值 */
  readPoint: (row: PointRow) => void | Promise<void>
  /** 打开编辑弹窗 */
  openEdit: (row: PointRow) => void
  /** 删除点位 */
  removePoint: (row: PointRow) => void | Promise<void>
  /** 切换写入白名单（el-switch 的 before-change 钩子） */
  toggleWritable: (row: PointRow) => Promise<boolean>
  /** 行内编辑保存（名称 / 地址 / 从站地址） */
  savePointField: (row: PointRow, patch: InlinePatch) => Promise<boolean>
}

/**
 * 模板展开点位的只读文本单元格（无配置元数据时显示占位符）。
 *
 * `.cell-readonly` 是 PointList.css 里的 scoped 样式：vnode 的 scopeId 取自
 * 「当前渲染实例」，而 cell 函数在 DataTable 的 CellView（已注入宿主 scopeId）内执行，
 * 因此把列定义放在独立模块也不影响样式命中。
 */
function readonlyCell(v: string | number | null | undefined) {
  return h('span', { class: 'cell-readonly' }, v ?? '—')
}

/** 点位清单列定义工厂 */
export function createPointColumns(actions: PointColumnActions): ColumnDef<PointRow>[] {
  const { canWrite, readPoint, openEdit, removePoint, toggleWritable, savePointField } = actions
  return [
    { key: 'id', title: '点位 ID', minWidth: 160, ellipsis: true, mono: true, sortable: true },
    {
      key: '_source',
      title: '来源',
      width: 80,
      // 模板展开点位不落 config.points，仅随运行时存在：标清来源，避免误以为可编辑
      cell: (row) =>
        row._source === 'template'
          ? h(ElTag, { type: 'info', size: 'small', effect: 'plain', round: true }, () => '模板')
          : h(ElTag, { type: 'success', size: 'small', effect: 'plain', round: true }, () => '配置'),
    },
    {
      key: 'name',
      title: '点位名称',
      width: 140,
      sortable: true,
      // 点位名称/点位地址/从站地址支持双击行内改：改个错别字不该再走一遍弹窗；模板行只读
      cell: (row) =>
        row._source === 'template'
          ? readonlyCell(row.name)
          : h(InlineEditCell, {
              value: row.name,
              label: '点位名称',
              maxlength: 64,
              save: (v: string | number) => savePointField(row, { name: String(v) }),
            }),
    },
    { key: 'bus', title: '总线', width: 90, sortable: true },
    protocolColumn<PointRow>({ width: 96, sortable: true }),
    {
      key: 'address',
      title: '点位地址',
      // 96 装得下 5 位地址但装不下「点位地址」+ 排序图标，会截成「点位地…」
      width: 108,
      sortable: true,
      cell: (row) =>
        row._source === 'template'
          ? readonlyCell(row.address || null)
          : h(InlineEditCell, {
              value: row.address,
              label: '点位地址',
              mono: true,
              align: 'right',
              maxlength: 64,
              save: (v: string | number) => savePointField(row, { address: String(v) }),
            }),
    },
    {
      key: 'slave_id',
      title: '从站地址',
      width: 76,
      sortable: true,
      cell: (row) =>
        row._source === 'template'
          ? readonlyCell(row.slave_id)
          : h(InlineEditCell, {
              value: row.slave_id,
              label: '从站地址',
              type: 'number',
              min: 1,
              max: 247,
              mono: true,
              align: 'right',
              save: (v: string | number) => savePointField(row, { slave_id: Number(v) }),
            }),
    },
    {
      key: 'ratio',
      title: '变比',
      width: 120,
      // 变比在表单里能配、列表里看不到时，现场无法一眼核对「读数是否需要放大」；
      // 模板展开的点位同样展示（只读），便于和设备铭牌比对。
      cell: (row) => {
        const parts: string[] = []
        if (row.ct_ratio != null) parts.push(`CT ${row.ct_ratio}`)
        if (row.pt_ratio != null) parts.push(`PT ${row.pt_ratio}`)
        return h('span', { class: 'cell-readonly' }, parts.length ? parts.join(' · ') : '—')
      },
    },
    {
      key: 'writable',
      title: '允许写入',
      width: 84,
      // S11: 白名单显式开关——before-change 返回 Promise，仅后端成功后才翻转
      // 模板行开关禁用：白名单归属配置，要改请在设备模板中调整
      cell: (row) =>
        h(ElSwitch, {
          modelValue: row.writable,
          size: 'small',
          disabled: row._source === 'template',
          beforeChange: () => toggleWritable(row),
        }),
    },
    actionColumn<PointRow>({
      // 固定在右缘（fixed: 'right'）：横向滚动时操作入口始终在手边，不会被滚出可视区。
      // 宽度收窄到 126px：编辑 + 「更多」两个入口足够，避免列宽挤压地址/从站地址。
      width: 126,
      fixed: 'right',
      // 高频操作（编辑）直接露出，读取/删除收进「更多」：
      // 3 个同权重文字按钮会把操作列撑到 260px，挤压地址、从站地址等信息列。
      // 按钮级权限：编辑/删除仅 point.write 可见；读取当前值所有角色可见。
      cell: (row) => {
        // 模板展开点位只读：只允许读取当前值，编辑/删除需改设备模板
        if (row._source === 'template') {
          return h('div', { class: 'row-actions' }, [
            h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => readPoint(row) },
              () => [h(ElIcon, null, () => h(Reading)), ' 读取当前值']),
          ])
        }
        const children = []
        if (canWrite()) {
          children.push(
            h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => openEdit(row) },
              () => [h(ElIcon, null, () => h(Edit)), ' 编辑']),
          )
        }
        // 「更多」下拉：至少有读取动作就显示；写权限时追加删除项
        const dropdownItems = [
          h(ElDropdownItem, { command: 'read' },
            () => [h(ElIcon, null, () => h(Reading)), ' 读取当前值']),
        ]
        if (canWrite()) {
          dropdownItems.push(h(ElDropdownItem, { command: 'delete', divided: true }, () => '删除点位'))
        }
        children.push(
          h(ElDropdown, {
            size: 'small',
            trigger: 'click',
            onCommand: (cmd: string) => (cmd === 'read' ? readPoint(row) : removePoint(row)),
          }, {
            default: () => h(ElButton, { size: 'small', type: 'primary', link: true },
              () => ['更多', h(ElIcon, { class: 'el-icon--right' }, () => h(ArrowDown))]),
            dropdown: () => h(ElDropdownMenu, null, () => dropdownItems),
          }),
        )
        return h('div', { class: 'row-actions' }, children)
      },
    }),
  ]
}
