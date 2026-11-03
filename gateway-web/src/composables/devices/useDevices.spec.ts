import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('@/api/devices', () => ({
  listDevices: vi.fn().mockResolvedValue([]),
  createDevice: vi.fn(),
  updateDevice: vi.fn(),
  deleteDevice: vi.fn(),
}))
vi.mock('@/api/protocolTools/slaveScan', () => ({
  scanBus: vi.fn().mockResolvedValue([]),
  respondedSlaves: (rows: Array<{ responded: boolean }>) => (rows ?? []).filter((r) => r.responded),
}))
vi.mock('@/api/points', () => ({
  listPoints: vi.fn().mockResolvedValue([]),
  readPointNow: vi.fn(),
}))
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
    ElNotification: vi.fn(),
  }
})
vi.mock('@/utils/confirm', () => ({
  confirmDelete: vi.fn(async () => true),
}))
vi.mock('vue-router', () => ({
  useRoute: vi.fn(() => ({ query: {} })),
}))

import { useDevices } from './useDevices'
import type { DeviceConfig } from '@/api/types'
import { listDevices, createDevice, deleteDevice, updateDevice } from '@/api/devices'
import { ElMessage, ElNotification } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'

describe('useDevices', () => {
  it('初始状态：devices=[], loading=false, 各 flag=false', () => {
    const { devices, loading, saving, testingConn, checking, editing } = useDevices()
    expect(devices.value).toEqual([])
    expect(loading.value).toBe(false)
    expect(saving.value).toBe(false)
    expect(testingConn.value).toBe(false)
    expect(checking.value).toBe(false)
    expect(editing.value).toBe(false)
  })

  it('初始 form 默认值为空/modbus', () => {
    const { form } = useDevices()
    expect(form.name).toBe('')
    expect(form.bus).toBe('')
    expect(form.protocol).toBe('modbus')
    expect(form.address).toBe('')
    expect(form.id).toBe('')
  })

  it('resetForm 清空表单字段并设 editing=false', () => {
    const { form, resetForm, editing } = useDevices()
    form.name = 'test'
    form.bus = 'bus1'
    form.protocol = 'dlt645'
    form.address = '5'
    editing.value = true
    resetForm()
    expect(form.name).toBe('')
    expect(form.bus).toBe('')
    expect(form.protocol).toBe('modbus')
    expect(form.address).toBe('')
    expect(editing.value).toBe(false)
  })

  it('addDevice 打开创建对话框 + editing=false', () => {
    const { addDevice, showCreate, editing } = useDevices()
    addDevice()
    expect(showCreate.value).toBe(true)
    expect(editing.value).toBe(false)
  })

  it('editDevice 打开编辑对话框 + 填充表单 + editing=true', () => {
    const { editDevice, showCreate, editing, form } = useDevices()
    const row: DeviceConfig = {
      id: 'd1',
      name: 'dev1',
      template: '',
      bus: 'bus1',
      protocol: 'dlt645',
      address: '3',
      slave_id: null,
      address_map: null,
      interval: 60,
    }
    editDevice(row)
    expect(showCreate.value).toBe(true)
    expect(editing.value).toBe(true)
    expect(form.id).toBe('d1')
    expect(form.name).toBe('dev1')
    expect(form.bus).toBe('bus1')
    expect(form.protocol).toBe('dlt645')
    expect(form.address).toBe('3')
  })

  it('onDiscoverAdd 从扫描结果填充表单', () => {
    const { onDiscoverAdd, form, showCreate } = useDevices()
    onDiscoverAdd({ address: '5', device_id: 'meter_5' } as Record<string, unknown>, 'rs485_a', 'modbus')
    expect(form.bus).toBe('rs485_a')
    expect(form.protocol).toBe('modbus')
    expect(form.name).toBe('meter_5')
    expect(showCreate.value).toBe(true)
  })

  it('onDiscoverAdd 无 device_id 时用 bus_address 兜底', () => {
    const { onDiscoverAdd, form } = useDevices()
    onDiscoverAdd({ address: '7' } as Record<string, unknown>, 'rs485_b', 'dlt645')
    expect(form.name).toBe('rs485_b_7')
    expect(form.bus).toBe('rs485_b')
    expect(form.protocol).toBe('dlt645')
  })

  it('commsQualityType 无统计返回 info', () => {
    const { commsQualityType } = useDevices()
    expect(commsQualityType('unknown_device')).toBe('info')
  })

  it('commsQualityText 无统计返回 —', () => {
    const { commsQualityText } = useDevices()
    expect(commsQualityText('unknown_device')).toBe('—')
  })

  describe('saveDeviceField（行内编辑）', () => {
    beforeEach(() => {
      vi.clearAllMocks()
    })

    const row: DeviceConfig & { group?: string; tags?: string[] } = {
      id: 'd1',
      name: '旧名',
      template: 'tpl1',
      bus: 'bus1',
      protocol: 'modbus',
      address: '40001',
      slave_id: 1,
      address_map: null,
      interval: 60,
      // 列表接口附带的扩展字段：PUT 整行覆盖时绝不能原样回传
      group: '车间A',
      tags: ['关键'],
    }

    it('成功后回写本地行并返回 true，只下发 DeviceConfig 字段', async () => {
      vi.mocked(updateDevice).mockResolvedValue(row)
      const { saveDeviceField, devices } = useDevices()
      devices.value = [row]

      const ok = await saveDeviceField(row, { name: '新名' })

      expect(ok).toBe(true)
      expect(row.name).toBe('新名')
      expect(updateDevice).toHaveBeenCalledTimes(1)
      const payload = vi.mocked(updateDevice).mock.calls[0]![1] as Record<string, unknown>
      expect(payload).toMatchObject({
        id: 'd1',
        name: '新名',
        bus: 'bus1',
        protocol: 'modbus',
        address: '40001',
        slave_id: 1,
        interval: 60,
      })
      // 扩展字段不能进 PUT 负载（后端可能按未知配置项拒绝）
      expect(payload).not.toHaveProperty('group')
      expect(payload).not.toHaveProperty('tags')
      // 不重载列表：否则页码复位、正在编辑的位置也会丢
      expect(listDevices).not.toHaveBeenCalled()
      expect(ElMessage.success).toHaveBeenCalledWith(expect.stringContaining('设备名'))
    })

    it('失败返回 false、保留原值，并给出可见提示', async () => {
      vi.mocked(updateDevice).mockRejectedValue(new Error('boom'))
      const { saveDeviceField } = useDevices()

      const ok = await saveDeviceField({ ...row, name: '旧名' }, { name: '新名' })

      expect(ok).toBe(false)
      expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('已保留原值'))
    })
  })

  describe('removeDevice 删除撤销', () => {
    beforeEach(() => {
      vi.clearAllMocks()
      vi.mocked(confirmDelete).mockResolvedValue(true)
      vi.mocked(listDevices).mockResolvedValue([])
    })

    const row: DeviceConfig = {
      id: 'd1',
      name: 'dev1',
      template: 'tpl1',
      bus: 'bus1',
      protocol: 'modbus',
      address: '1',
      slave_id: 1,
      address_map: null,
      interval: 60,
    }

    /** 从 ElNotification 调用参数中取出「撤销删除」按钮的 onClick（VNode 树结构固定） */
    function getUndoClick(): () => void {
      expect(ElNotification).toHaveBeenCalledTimes(1)
      const opts = vi.mocked(ElNotification).mock.calls[0]![0] as unknown as {
        message: { children: { props: { onClick: () => void } }[] }
      }
      return opts.message.children[1]!.props.onClick
    }

    it('确认后删除成功：弹含撤销按钮的通知并刷新列表', async () => {
      vi.mocked(deleteDevice).mockResolvedValue({ status: 'ok' })
      const { removeDevice } = useDevices()

      await removeDevice(row)

      expect(deleteDevice).toHaveBeenCalledWith('d1')
      expect(ElNotification).toHaveBeenCalledTimes(1)
      expect(listDevices).toHaveBeenCalled()
    })

    it('点撤销：用删除前快照重新创建（仅 DeviceConfig 字段）', async () => {
      vi.mocked(deleteDevice).mockResolvedValue({ status: 'ok' })
      vi.mocked(createDevice).mockResolvedValue(row)
      const { removeDevice } = useDevices()
      await removeDevice(row)

      getUndoClick()()
      await vi.waitFor(() => expect(createDevice).toHaveBeenCalledTimes(1))

      const payload = vi.mocked(createDevice).mock.calls[0]![0] as Record<string, unknown>
      expect(payload).toMatchObject({ id: 'd1', name: 'dev1', bus: 'bus1', protocol: 'modbus', interval: 60 })
      expect(ElMessage.success).toHaveBeenCalledWith(expect.stringContaining('已恢复'))
      // 恢复后刷新列表
      expect(listDevices).toHaveBeenCalledTimes(2)
    })

    it('用户在确认框点取消：不删除、不弹通知', async () => {
      vi.mocked(confirmDelete).mockResolvedValue(false)
      const { removeDevice } = useDevices()

      await removeDevice(row)

      expect(deleteDevice).not.toHaveBeenCalled()
      expect(ElNotification).not.toHaveBeenCalled()
    })

    it('删除接口失败：错误提示，不弹撤销通知', async () => {
      vi.mocked(deleteDevice).mockRejectedValue(new Error('boom'))
      const { removeDevice } = useDevices()

      await removeDevice(row)

      // 失败原因必须透出：只提示「请稍后重试」会让用户无从判断是权限、配额还是冲突
      expect(ElMessage.error).toHaveBeenCalledWith('删除设备失败：boom')
      expect(ElNotification).not.toHaveBeenCalled()
    })
  })
})
