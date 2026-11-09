import { describe, it, expect, beforeEach, vi } from 'vitest'

/** 确认框与常驻通知：批量删除的交互结果需要可观测 */
const m = vi.hoisted(() => ({ confirmDelete: vi.fn(), notify: vi.fn() }))

vi.mock('@/utils/confirm', () => ({ confirmDelete: (...a: unknown[]) => m.confirmDelete(...a) }))

vi.mock('@/api/points', () => ({
  listPoints: vi.fn().mockResolvedValue([]),
  deletePoint: vi.fn(),
  restorePoint: vi.fn(),
  readPointNow: vi.fn(),
  batchImportPoints: vi.fn(),
  createPoint: vi.fn(),
  updatePoint: vi.fn(),
}))
vi.mock('@/api/devices', () => ({
  listDevices: vi.fn().mockResolvedValue([]),
}))
vi.mock('@/api/dataQuality', () => ({
  listPointQuality: vi.fn().mockResolvedValue([]),
}))
// 北向上报周期由配置接口提供：点位采集周期必须小于它
vi.mock('@/api/config', () => ({
  getConfig: vi.fn().mockResolvedValue({ mqtt: { enabled: true, publish_interval: 60 } }),
}))
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
    ElNotification: m.notify,
  }
})

import { usePoints } from './usePoints'
import { updatePoint, listPoints, deletePoint, restorePoint, createPoint } from '@/api/points'
import { getConfig } from '@/api/config'
import { ElMessage, ElButton, type FormInstance } from 'element-plus'
import { nextTick } from 'vue'
import type { VNode } from 'vue'
import type { PointConfig, DeviceConfig } from '@/api/types'

/** 从常驻通知的 message vnode 树中取出第一个按钮（撤销 / 全部撤销） */
function findButton(vnode: VNode): VNode | undefined {
  if ((vnode.type as unknown) === ElButton) return vnode
  const children = Array.isArray(vnode.children) ? (vnode.children as VNode[]) : []
  for (const child of children) {
    if (child && typeof child === 'object') {
      const found = findButton(child as VNode)
      if (found) return found
    }
  }
  return undefined
}

/** 触发按钮的 onClick（常驻通知里的撤销入口） */
async function clickButton(vnode: VNode): Promise<void> {
  const onClick = vnode.props?.onClick as (() => unknown) | undefined
  await onClick?.()
}

/** 取第 index 次 ElNotification 的 message vnode（mock 参数为 unknown，需显式收窄） */
function noticeMessage(index: number): VNode {
  const args = m.notify.mock.calls[index] as unknown[]
  return (args[0] as { message: VNode }).message
}

/** 递归拼接 vnode 树的纯文本，用于校验失败 id 确实被列出 */
function collectText(vnode: VNode): string {
  const own = typeof vnode.children === 'string' ? vnode.children : ''
  const childText = Array.isArray(vnode.children)
    ? (vnode.children as VNode[])
      .filter((c) => c && typeof c === 'object')
      .map((c) => collectText(c as VNode))
      .join('')
    : ''
  return own + childText
}

/** 最小合法点位配置：满足 PointConfig 全部必填字段（测试用 overrides 覆盖关注项） */
function makePoint(overrides: Partial<PointConfig> = {}): PointConfig {
  return {
    id: '',
    name: null,
    device_id: null,
    bus: '',
    protocol: 'modbus',
    address: '',
    slave_id: null,
    address_num: null,
    data_id: null,
    function_code: null,
    data_type: null,
    byte_order: null,
    register_count: null,
    interval: 60,
    scale: 1,
    offset: 0,
    formula: null,
    ct_ratio: null,
    pt_ratio: null,
    deadband: 0,
    on_change: false,
    unit: null,
    alarm_high: null,
    alarm_low: null,
    aggregation: null,
    modbus_register: null,
    // S11: 写入白名单标记（默认拒绝写入）
    writable: false,
    ...overrides,
  }
}

/** 最小设备配置：满足 DeviceConfig 全部字段（测试用 overrides 覆盖关注项） */
function makeDevice(overrides: Partial<DeviceConfig> = {}): DeviceConfig {
  return {
    id: 'd1',
    name: 'dev1',
    template: 'tpl',
    bus: 'bus1',
    protocol: 'modbus',
    address: '1',
    slave_id: null,
    address_map: null,
    interval: 10,
    ...overrides,
  }
}

describe('usePoints', () => {
  it('初始状态：devices=[], points=[], loading=false, selectedDevice=""', () => {
    const { devices, points, loading, selectedDevice } = usePoints()
    expect(devices.value).toEqual([])
    expect(points.value).toEqual([])
    expect(loading.value).toBe(false)
    expect(selectedDevice.value).toBe('')
  })

  it('mergedPoints：模板展开点位以只读行并入，同 id 以配置点位为准，他设备不混入', () => {
    const { devices, points, runtimePoints, mergedPoints, selectedDevice } = usePoints()
    devices.value = [makeDevice({ id: 'd1' })]
    selectedDevice.value = 'd1'
    points.value = [makePoint({ id: 'cfg1', name: '配置点位' })]
    runtimePoints.value = [
      // 同 id：配置点位已在清单中，运行时条目不重复出现
      { point_id: 'cfg1', point_name: '运行时名', device_id: 'd1', device_name: 'dev1', value: 1, quality: 'good', consecutive_failures: 0, last_update_at: 0 },
      { point_id: 'rt1', point_name: '模板点-温度', device_id: 'd1', device_name: 'dev1', value: 25, quality: 'good', consecutive_failures: 0, last_update_at: 0 },
      // 其他设备的运行时点位不并入当前清单
      { point_id: 'rt2', point_name: '他设备点位', device_id: 'd2', device_name: 'dev2', value: 1, quality: 'good', consecutive_failures: 0, last_update_at: 0 },
    ]
    const ids = mergedPoints.value.map((p) => p.id)
    expect(ids).toEqual(['cfg1', 'rt1'])
    const tpl = mergedPoints.value.find((p) => p.id === 'rt1')
    expect(tpl?._source).toBe('template')
    // 模板行继承设备的总线/协议（运行时不含这些配置字段）
    expect(tpl?.bus).toBe('bus1')
    expect(tpl?.protocol).toBe('modbus')
    expect(tpl?.writable).toBe(false)
    // 配置点位无 _source 标记
    expect(mergedPoints.value[0]?._source).toBeUndefined()
  })

  /**
   * 回归：切换设备竞态。原实现 await 之后直接写 points.value，没有请求序号守卫，
   * A→B 快切时若 A 的响应后到，会用 A 的点位覆盖 B 的列表（表格与下拉框不一致，
   * 复制/删除等操作会作用在错误设备上）。
   */
  it('切换设备竞态：旧设备响应后到时不得覆盖新设备点位', async () => {
    const { points, selectedDevice, loadPoints } = usePoints()
    const mocked = vi.mocked(listPoints)

    let resolveA!: (v: PointConfig[]) => void
    let resolveB!: (v: PointConfig[]) => void
    mocked.mockImplementationOnce(() => new Promise<PointConfig[]>((r) => { resolveA = r }))
    mocked.mockImplementationOnce(() => new Promise<PointConfig[]>((r) => { resolveB = r }))

    selectedDevice.value = 'A'
    const pendingA = loadPoints()
    selectedDevice.value = 'B'
    const pendingB = loadPoints()

    // B 先返回，A 后返回：A 的结果必须被丢弃
    resolveB([makePoint({ id: 'b1' })])
    await pendingB
    resolveA([makePoint({ id: 'a1' })])
    await pendingA

    expect(points.value.map((p) => p.id)).toEqual(['b1'])
  })

  it('初始 pointForm 默认值', () => {
    const { pointForm } = usePoints()
    expect(pointForm.id).toBe('')
    expect(pointForm.name).toBe('')
    expect(pointForm.bus).toBe('')
    expect(pointForm.protocol).toBe('modbus')
    expect(pointForm.address).toBe('')
    expect(pointForm.slave_id).toBe(1)
    expect(pointForm.address_num).toBe(0)
  })

  it('resetForm 清空表单', () => {
    const { pointForm, resetForm } = usePoints()
    pointForm.id = 'p1'
    pointForm.name = '点位1'
    pointForm.bus = 'bus1'
    pointForm.protocol = 'dlt645'
    pointForm.address = '1234'
    pointForm.slave_id = 5
    pointForm.address_num = 100
    resetForm()
    expect(pointForm.id).toBe('')
    expect(pointForm.name).toBe('')
    expect(pointForm.bus).toBe('')
    expect(pointForm.protocol).toBe('modbus')
    expect(pointForm.address).toBe('')
    expect(pointForm.slave_id).toBe(1)
    expect(pointForm.address_num).toBe(0)
  })

  it('告警高低限交叉校验：低限不得大于等于高限', async () => {
    const { pointForm, pointRules } = usePoints()
    // 直接驱动规则校验器：提交时的 el-form.validate 最终走的就是它
    const rules = pointRules.alarm_high as unknown as Array<{
      validator: (rule: unknown, value: unknown, cb: (e?: Error) => void) => void
    }>
    const run = () =>
      new Promise<Error | undefined>((resolve) => {
        rules[0]!.validator({}, pointForm.alarm_high, (e) => resolve(e))
      })

    pointForm.alarm_high = 50
    pointForm.alarm_low = 60
    expect((await run())?.message).toBe('低限必须小于高限')

    // 仅一侧填写时不校验（表示该侧不启用）
    pointForm.alarm_low = undefined
    expect(await run()).toBeUndefined()

    pointForm.alarm_low = 40
    expect(await run()).toBeUndefined()
  })

  it('openCreate 设 formMode=create + showForm=true', () => {
    const { openCreate, formMode, showForm, selectedDevice } = usePoints()
    selectedDevice.value = 'd1'
    openCreate()
    expect(formMode.value).toBe('create')
    expect(showForm.value).toBe(true)
  })

  it('openCreate 无 selectedDevice 时不开窗', () => {
    const { openCreate, showForm } = usePoints()
    openCreate()
    expect(showForm.value).toBe(false)
  })

  it('openEdit 设 formMode=edit + 填充表单', () => {
    const { openEdit, formMode, showForm, pointForm } = usePoints()
    openEdit(
      makePoint({
        id: 'p1',
        name: '温度',
        bus: 'bus1',
        protocol: 'modbus',
        address: '40001',
        slave_id: 2,
        address_num: 10,
      }),
    )
    expect(formMode.value).toBe('edit')
    expect(showForm.value).toBe(true)
    expect(pointForm.id).toBe('p1')
    expect(pointForm.name).toBe('温度')
    expect(pointForm.bus).toBe('bus1')
    expect(pointForm.protocol).toBe('modbus')
    // 展示地址按「功能码 + 偏移」重新推导：fc 缺省 3（保持寄存器）+ 偏移 10 → 30011。
    // 原样保留 40001 会与真实寻址不一致（实际读的是 FC03 的 10 号寄存器）。
    expect(pointForm.address).toBe('30011')
    expect(pointForm.slave_id).toBe(2)
    expect(pointForm.address_num).toBe(10)
  })

  /** 后端 PUT 会无条件覆盖这些字段：不回填 = 用户打开编辑点保存就把变比/寻址参数清空 */
  it('openEdit 回填 CT/PT 变比与 Modbus 解码参数', () => {
    const { openEdit, pointForm } = usePoints()
    openEdit(
      makePoint({
        id: 'p3',
        bus: 'bus1',
        protocol: 'modbus',
        address: '30001',
        address_num: 0,
        function_code: 4,
        data_type: 'float32',
        byte_order: 'CDAB',
        register_count: 2,
        unit: 'kWh',
        ct_ratio: 30,
        pt_ratio: 100,
      }),
    )
    expect(pointForm.function_code).toBe(4)
    // 区段 4xxxx → FC04，偏移 0 → 40001
    expect(pointForm.address).toBe('40001')
    expect(pointForm.data_type).toBe('float32')
    expect(pointForm.byte_order).toBe('CDAB')
    expect(pointForm.register_count).toBe(2)
    expect(pointForm.ct_ratio).toBe(30)
    expect(pointForm.pt_ratio).toBe(100)
  })

  it('切换单位后清空不生效的一侧变比（电压只留 PT、电流只留 CT、功率因数都清）', async () => {
    const { pointForm } = usePoints()
    pointForm.unit = 'kWh'
    pointForm.ct_ratio = 30
    pointForm.pt_ratio = 100
    await nextTick()
    expect(pointForm.ct_ratio).toBe(30)
    expect(pointForm.pt_ratio).toBe(100)

    pointForm.unit = 'A'
    await nextTick()
    expect(pointForm.ct_ratio).toBe(30)
    expect(pointForm.pt_ratio).toBeNull()

    pointForm.unit = 'PF'
    await nextTick()
    expect(pointForm.ct_ratio).toBeNull()
    expect(pointForm.pt_ratio).toBeNull()
  })

  it('提交时按单位决定变比：电压只提 PT、功率因数都不提', async () => {
    vi.mocked(createPoint).mockReset()
    vi.mocked(createPoint).mockResolvedValue(makePoint())
    const { pointForm, pointFormRef, submitPoint, selectedDevice } = usePoints()
    pointFormRef.value = { validate: () => Promise.resolve(true) } as unknown as FormInstance
    selectedDevice.value = 'd1'

    pointForm.id = 'p1'
    pointForm.protocol = 'modbus'
    pointForm.function_code = 4
    pointForm.address_num = 0
    pointForm.address = '40001'
    pointForm.unit = 'V'
    pointForm.ct_ratio = 30
    pointForm.pt_ratio = 100
    await submitPoint()
    expect(createPoint).toHaveBeenCalledWith('d1', expect.objectContaining({
      ct_ratio: null,
      pt_ratio: 100,
      function_code: 4,
      address_num: 0,
    }))

    vi.mocked(createPoint).mockClear()
    pointForm.unit = 'PF'
    pointForm.ct_ratio = 30
    pointForm.pt_ratio = 100
    await submitPoint()
    expect(createPoint).toHaveBeenCalledWith('d1', expect.objectContaining({
      ct_ratio: null,
      pt_ratio: null,
    }))
  })

  it('openEdit 缺少 slave_id/address_num 时用默认值', () => {
    const { openEdit, pointForm } = usePoints()
    openEdit(
      makePoint({
        id: 'p2',
        name: '',
        bus: 'bus2',
        protocol: 'dlt645',
        address: '00000001',
      }),
    )
    expect(pointForm.slave_id).toBe(1)
    expect(pointForm.address_num).toBe(0)
  })

  it('onSelectionChange 更新 selection', () => {
    const { onSelectionChange, selection } = usePoints()
    const rows: PointConfig[] = [makePoint({ id: 'p1' }), makePoint({ id: 'p2' })]
    onSelectionChange(rows)
    expect(selection.value).toEqual(rows)
  })

  it('currentDeviceName 无选中设备返回 —', () => {
    const { currentDeviceName } = usePoints()
    expect(currentDeviceName.value).toBe('—')
  })

  describe('savePointField（行内编辑）', () => {
    beforeEach(() => {
      vi.mocked(updatePoint).mockReset()
      vi.mocked(ElMessage.success).mockClear()
      vi.mocked(ElMessage.error).mockClear()
    })

    it('成功后回写本地行并返回 true，且不重载列表（保留页码与勾选）', async () => {
      vi.mocked(updatePoint).mockResolvedValue(makePoint({ id: 'p1', name: '新名' }))
      const { points, savePointField } = usePoints()
      const row = makePoint({ id: 'p1', name: '旧名', interval: 30 })
      points.value = [row]

      const ok = await savePointField(row, { name: '新名' })

      expect(ok).toBe(true)
      // PUT 会覆盖未传字段：必须携带整行配置 + 合法 interval
      expect(updatePoint).toHaveBeenCalledWith('p1', expect.objectContaining({
        id: 'p1',
        name: '新名',
        bus: '',
        protocol: 'modbus',
        interval: 30,
      }))
      expect(row.name).toBe('新名')
      expect(listPoints).not.toHaveBeenCalled()
      expect(ElMessage.success).toHaveBeenCalledWith(expect.stringContaining('名称'))
    })

    it('编辑大周期点位不会被静默改写（不再按 59s 截断）', async () => {
      vi.mocked(updatePoint).mockResolvedValue(makePoint({ id: 'p1', name: '新名', interval: 300 }))
      const { points, savePointField } = usePoints()
      const row = makePoint({ id: 'p1', name: '旧名', interval: 300 })
      points.value = [row]

      await savePointField(row, { name: '新名' })

      // 旧实现 clampInterval 硬编码上界 59：仅改个名字就把 300s 采集周期改成 59s
      expect(updatePoint).toHaveBeenCalledWith('p1', expect.objectContaining({ interval: 300 }))
    })

    it('失败返回 false、保留原值，并给出可见提示', async () => {
      vi.mocked(updatePoint).mockRejectedValue(new Error('boom'))
      const { savePointField } = usePoints()
      const row = makePoint({ id: 'p1', name: '旧名' })

      const ok = await savePointField(row, { name: '新名' })

      expect(ok).toBe(false)
      expect(row.name).toBe('旧名')
      expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('已保留原值'))
    })
  })

  describe('batchDelete（结果聚合）', () => {
    beforeEach(() => {
      m.confirmDelete.mockReset()
      m.notify.mockReset()
      vi.mocked(deletePoint).mockReset()
      vi.mocked(restorePoint).mockReset()
    })

    it('全部成功：返回汇总数量并给出成功提示 + 可撤销通知', async () => {
      m.confirmDelete.mockResolvedValue(true)
      vi.mocked(deletePoint).mockResolvedValue({ status: 'ok' })
      vi.mocked(ElMessage.success).mockClear()

      const { selection, batchDelete } = usePoints()
      selection.value = [makePoint({ id: 'p1' }), makePoint({ id: 'p2' })]
      const res = await batchDelete()

      expect(res).toEqual({ deleted: 2, failed: [] })
      expect(ElMessage.success).toHaveBeenCalledWith(expect.stringContaining('2'))
      // 软删除后可整体撤销：成功后必须给一条带「全部撤销」的通知
      expect(m.notify).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'success', duration: 8000 }),
      )
    })

    it('部分失败：逐条收集 id + 原因，并用常驻通知列出（不能只报 N 个失败）', async () => {
      m.confirmDelete.mockResolvedValue(true)
      vi.mocked(deletePoint).mockImplementation((id: string) =>
        id === 'p2'
          ? Promise.reject(new Error('设备忙'))
          : Promise.resolve({ status: 'ok' }),
      )
      vi.mocked(ElMessage.success).mockClear()

      const { selection, batchDelete } = usePoints()
      selection.value = [
        makePoint({ id: 'p1' }),
        makePoint({ id: 'p2' }),
        makePoint({ id: 'p3' }),
      ]
      const res = await batchDelete()

      expect(res).toEqual({ deleted: 2, failed: [{ id: 'p2', reason: '设备忙' }] })
      // 失败项必须能被用户定位到具体点位，否则"重试哪几条"无从判断
      expect(m.notify).toHaveBeenCalledTimes(1)
      expect(m.notify).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'warning',
          duration: 0,
          title: expect.stringContaining('1 个点位未删除'),
        }),
      )
      // 部分失败时不弹"已全部删除"的成功提示
      expect(ElMessage.success).not.toHaveBeenCalled()
    })

    it('用户取消确认：不发起任何删除、返回 undefined', async () => {
      m.confirmDelete.mockResolvedValue(false)
      const { selection, batchDelete } = usePoints()
      selection.value = [makePoint({ id: 'p1' })]
      const res = await batchDelete()

      expect(res).toBeUndefined()
      expect(deletePoint).not.toHaveBeenCalled()
    })
  })

  describe('软删除撤销（回收站恢复）', () => {
    beforeEach(() => {
      m.confirmDelete.mockReset()
      m.notify.mockReset()
      vi.mocked(deletePoint).mockReset()
      vi.mocked(restorePoint).mockReset()
      vi.mocked(listPoints).mockClear()
      vi.mocked(ElMessage.success).mockClear()
      vi.mocked(ElMessage.error).mockClear()
    })

    it('单条删除成功后出现「撤销」入口，点击调用 restorePoint 并重载列表', async () => {
      m.confirmDelete.mockResolvedValue(true)
      vi.mocked(deletePoint).mockResolvedValue({ status: 'ok' })
      vi.mocked(restorePoint).mockResolvedValue(null)

      const { selectedDevice, removePoint } = usePoints()
      selectedDevice.value = 'd1'
      await removePoint(makePoint({ id: 'p1' }))

      expect(deletePoint).toHaveBeenCalledWith('p1')
      expect(m.notify).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'success',
          duration: 8000,
          title: expect.stringContaining('p1'),
        }),
      )

      const btn = findButton(noticeMessage(0))
      expect(btn).toBeDefined()
      await clickButton(btn as VNode)

      expect(restorePoint).toHaveBeenCalledWith('p1')
      expect(ElMessage.success).toHaveBeenCalledWith('已恢复')
      // 删除后 + 恢复后各重载一次
      expect(vi.mocked(listPoints).mock.calls.length).toBe(2)
    })

    it('恢复失败时给出可见提示且不重载列表', async () => {
      m.confirmDelete.mockResolvedValue(true)
      vi.mocked(deletePoint).mockResolvedValue({ status: 'ok' })
      vi.mocked(restorePoint).mockRejectedValue(new Error('id 冲突'))

      const { selectedDevice, removePoint } = usePoints()
      selectedDevice.value = 'd1'
      await removePoint(makePoint({ id: 'p1' }))

      const btn = findButton(noticeMessage(0))
      await clickButton(btn as VNode)

      expect(ElMessage.error).toHaveBeenCalledWith('恢复失败，请到「已删除点位」重试')
    })

    it('批量删除「全部撤销」并发恢复：部分失败时追加 warning 通知列出失败 id', async () => {
      m.confirmDelete.mockResolvedValue(true)
      vi.mocked(deletePoint).mockResolvedValue({ status: 'ok' })
      vi.mocked(restorePoint).mockImplementation((id: string) =>
        id === 'p2' ? Promise.reject(new Error('id 冲突')) : Promise.resolve(null),
      )

      const { selection, selectedDevice, batchDelete } = usePoints()
      selectedDevice.value = 'd1'
      selection.value = [
        makePoint({ id: 'p1' }),
        makePoint({ id: 'p2' }),
        makePoint({ id: 'p3' }),
      ]
      await batchDelete()

      expect(m.notify).toHaveBeenCalledTimes(1)
      const btn = findButton(noticeMessage(0))
      expect(btn).toBeDefined()
      await clickButton(btn as VNode)

      // 三条全部尝试恢复（并发上限 8）
      expect(restorePoint).toHaveBeenCalledTimes(3)
      // 第二条通知：warning + 常驻 + 列出失败 id
      expect(m.notify).toHaveBeenCalledTimes(2)
      expect(m.notify).toHaveBeenNthCalledWith(2,
        expect.objectContaining({
          type: 'warning',
          duration: 0,
          title: expect.stringContaining('1 个点位恢复失败'),
        }),
      )
      expect(collectText(noticeMessage(1))).toContain('p2')
      expect(vi.mocked(listPoints).mock.calls.length).toBe(2)
    })
  })

  describe('toggleWritable（S11: 点位行显式白名单开关）', () => {
    it('成功：翻转 writable、回传整行配置、返回 true', async () => {
      vi.mocked(updatePoint).mockResolvedValueOnce(makePoint({ id: 'p1', writable: true }))
      const { toggleWritable } = usePoints()
      const row = makePoint({ id: 'p1', bus: 'bus1', address: '40001', writable: false })
      const ok = await toggleWritable(row)
      expect(ok).toBe(true)
      expect(row.writable).toBe(true)
      expect(updatePoint).toHaveBeenCalledWith(
        'p1',
        expect.objectContaining({ id: 'p1', bus: 'bus1', address: '40001', writable: true }),
      )
    })

    it('失败：保持原状、返回 false，且必须给用户可见提示（写操作全局静默）', async () => {
      vi.mocked(updatePoint).mockRejectedValueOnce(new Error('400'))
      vi.mocked(ElMessage.error).mockClear()
      const { toggleWritable } = usePoints()
      const row = makePoint({ id: 'p2', writable: false })
      const ok = await toggleWritable(row)
      expect(ok).toBe(false)
      expect(row.writable).toBe(false)
      // 拦截器对 PUT 只留痕不弹窗，这里不提示就等于用户点了没反应
      expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('写入白名单切换失败'))
    })
  })

  describe('doImport（批量导入实时进度）', () => {
    const importJson = JSON.stringify([
      { id: 'p1', bus: 'bus1', protocol: 'modbus', address: '40001', interval: 1, on_change: false },
      { id: 'p2', bus: 'bus1', protocol: 'modbus', address: '40002', interval: 1, on_change: false },
      { id: 'p3', bus: 'bus1', protocol: 'modbus', address: '40003', interval: 1, on_change: false },
    ])

    /** 等待已 resolve/reject 的 createPoint 走完 worker 的 finally */
    const tick = () => new Promise((r) => setTimeout(r, 0))

    beforeEach(() => {
      vi.mocked(createPoint).mockReset()
      m.notify.mockReset()
      vi.mocked(ElMessage.success).mockClear()
      vi.mocked(ElMessage.error).mockClear()
    })

    it('worker 每完成一条 completed++（含失败），开始/结束时 running 为 true/false，total 为点位总数', async () => {
      const resolvers: Array<(v: PointConfig) => void> = []
      const rejecters: Array<(e: Error) => void> = []
      vi.mocked(createPoint).mockImplementation(() => new Promise<PointConfig>((res, rej) => {
        resolvers.push(res)
        rejecters.push(rej)
      }))

      const { devices, selectedDevice, importText, importProgress, importing, doImport } = usePoints()
      devices.value = [makeDevice()]
      selectedDevice.value = 'd1'
      importText.value = importJson

      const pending = doImport()

      // 状态在第一条请求发出前就已置位：UI 才能在点击后立刻显示进度条
      expect(importProgress.running).toBe(true)
      expect(importProgress.total).toBe(3)
      expect(importProgress.completed).toBe(0)
      expect(importing.value).toBe(true)
      expect(createPoint).toHaveBeenCalledTimes(3)

      resolvers[0]!(makePoint({ id: 'p1' }))
      await tick()
      expect(importProgress.completed).toBe(1)
      expect(importProgress.running).toBe(true)

      rejecters[1]!(new Error('设备忙'))
      await tick()
      // 失败也必须计入完成数，否则进度条会永远卡在 2/3
      expect(importProgress.completed).toBe(2)
      expect(importProgress.running).toBe(true)

      resolvers[2]!(makePoint({ id: 'p3' }))
      const res = await pending

      expect(res).toEqual({ created: 2, failed: [{ id: 'p2', reason: '设备忙' }] })
      // 结束后保留 completed/total 供结果展示，running 归位
      expect(importProgress).toMatchObject({ completed: 3, total: 3, running: false })
      expect(importing.value).toBe(false)
    })

    it('导入点位继承目标设备的总线与协议，跨设备粘贴 JSON 不再全部失败', async () => {
      vi.mocked(createPoint).mockResolvedValue(makePoint())

      const { devices, selectedDevice, importText, doImport } = usePoints()
      // 目标设备在 bus2，粘贴的是从 bus1 设备导出的点位
      devices.value = [makeDevice({ bus: 'bus2', protocol: 'modbus' })]
      selectedDevice.value = 'd1'
      importText.value = JSON.stringify([
        { id: 'p1', name: '温度', bus: 'bus1', protocol: 'modbus', address: '40001', interval: 1, on_change: false },
      ])

      const res = await doImport()

      expect(res).toEqual({ created: 1, failed: [] })
      // 后端 validate_point_inherits_device 要求 bus/protocol 与设备一致：
      // 只改 device_id 会让每条点位都 4002，这正是「批量导入全部失败」的根因
      expect(createPoint).toHaveBeenCalledWith('d1', expect.objectContaining({
        bus: 'bus2',
        protocol: 'modbus',
        device_id: 'd1',
      }))
    })

    it('导入时保留 name/scale/unit/data_type 等字段，INTERVAL 收敛到后端合法区间', async () => {
      vi.mocked(createPoint).mockResolvedValue(makePoint())

      const { devices, selectedDevice, importText, doImport } = usePoints()
      devices.value = [makeDevice()]
      selectedDevice.value = 'd1'
      importText.value = JSON.stringify([
        {
          id: 'p1', name: '进线电压', bus: 'bus1', protocol: 'modbus', address: '40001',
          interval: 60, on_change: false, scale: 0.1, unit: 'V', data_type: 'u16',
        },
      ])

      await doImport()

      // 旧实现只挑 8 个字段，导入后点位变成无名、量程失效的空壳
      expect(createPoint).toHaveBeenCalledWith('d1', expect.objectContaining({
        name: '进线电压',
        scale: 0.1,
        unit: 'V',
        data_type: 'u16',
        interval: 59,
      }))
    })

    it('导入周期的上界跟随北向上报周期，而不是硬编码 59s', async () => {
      vi.mocked(getConfig).mockResolvedValue({
        mqtt: { enabled: true, publish_interval: 300 },
      } as unknown as Awaited<ReturnType<typeof getConfig>>)
      vi.mocked(createPoint).mockResolvedValue(makePoint())

      const { northInterval, loadDevices, devices, selectedDevice, importText, doImport } = usePoints()
      // 北向周期在 loadDevices 中异步拉取，等它落定再导入
      await loadDevices()
      await vi.waitFor(() => expect(northInterval.value).toBe(300))

      devices.value = [makeDevice()]
      selectedDevice.value = 'd1'
      importText.value = JSON.stringify([
        { id: 'p1', bus: 'bus1', protocol: 'modbus', address: '40001', interval: 120, on_change: false },
      ])

      await doImport()

      expect(createPoint).toHaveBeenCalledWith('d1', expect.objectContaining({ interval: 120 }))

      // 复位，避免影响后续用例
      vi.mocked(getConfig).mockResolvedValue({
        mqtt: { enabled: true, publish_interval: 60 },
      } as unknown as Awaited<ReturnType<typeof getConfig>>)
    })

    it('下一次导入开始时重置 completed/total', async () => {
      vi.mocked(createPoint).mockResolvedValue(makePoint())

      const { devices, selectedDevice, importText, importProgress, doImport } = usePoints()
      devices.value = [makeDevice()]
      selectedDevice.value = 'd1'
      importText.value = importJson

      await doImport()
      expect(importProgress).toMatchObject({ completed: 3, total: 3, running: false })

      importText.value = JSON.stringify([
        { id: 'p9', bus: 'bus1', protocol: 'modbus', address: '40009', interval: 1, on_change: false },
      ])
      const pending = doImport()

      expect(importProgress.total).toBe(1)
      expect(importProgress.completed).toBe(0)
      expect(importProgress.running).toBe(true)

      await pending
      expect(importProgress).toMatchObject({ completed: 1, total: 1, running: false })
    })
  })
})
