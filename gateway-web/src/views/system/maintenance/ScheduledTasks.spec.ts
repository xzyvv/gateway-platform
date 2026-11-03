// ScheduledTasks.vue 单元测试：列表/新建/下次执行时间渲染/立即执行/停用/删除。
// mock 边界同仓内其他页面测试：@/api/scheduledTasks 全量 mock、confirmDelete 桩、ElMessage 桩。
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import type { ScheduleTask } from '@/api/scheduledTasks'
import { formatDateTime } from '@/utils/datetime'

const {
  listTasksMock,
  createTaskMock,
  updateTaskMock,
  deleteTaskMock,
  runTaskNowMock,
  confirmDeleteMock,
} = vi.hoisted(() => ({
  listTasksMock: vi.fn(),
  createTaskMock: vi.fn(),
  updateTaskMock: vi.fn(),
  deleteTaskMock: vi.fn(),
  runTaskNowMock: vi.fn(),
  confirmDeleteMock: vi.fn(),
}))

vi.mock('@/api/scheduledTasks', () => ({
  listTasks: listTasksMock,
  createTask: createTaskMock,
  updateTask: updateTaskMock,
  deleteTask: deleteTaskMock,
  runTaskNow: runTaskNowMock,
  // 与 src/api/scheduledTasks.ts 的 CRON_PRESETS 同形（值不影响断言）
  getCronPresets: () => [
    { label: '每分钟', value: '* * * * *' },
    { label: '每5分钟', value: '*/5 * * * *' },
    { label: '每15分钟', value: '*/15 * * * *' },
    { label: '每小时', value: '0 * * * *' },
    { label: '每天2点', value: '0 2 * * *' },
    { label: '每周一', value: '0 0 * * 1' },
  ],
}))

vi.mock('@/utils/confirm', () => ({
  confirmDelete: confirmDeleteMock,
  confirmAction: vi.fn(),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

import ScheduledTasks from './ScheduledTasks.vue'
import { ElMessage } from 'element-plus'

const TASK: ScheduleTask = {
  id: 't1',
  name: '每日备份',
  type: 'backup_config',
  cron: '0 2 * * *',
  enabled: true,
  nextRunAt: 1780000000000,
  runCount: 5,
  lastStatus: 'success',
  createdAt: 1770000000000,
}

const wrappers: ReturnType<typeof mount>[] = []

afterEach(() => {
  for (const w of wrappers) w.unmount()
  wrappers.length = 0
})

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  const wrapper = mount(ScheduledTasks)
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

async function findButton(wrapper: Awaited<ReturnType<typeof mountPage>>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().includes(text))
  expect(btn, `未找到「${text}」按钮`).toBeTruthy()
  return btn!
}

describe('ScheduledTasks.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listTasksMock.mockResolvedValue([TASK])
    deleteTaskMock.mockResolvedValue(undefined)
    confirmDeleteMock.mockResolvedValue(true)
  })

  it('挂载后加载任务列表并渲染行：nextRunAt 为可读时间而非 Invalid Date', async () => {
    const wrapper = await mountPage()

    expect(listTasksMock).toHaveBeenCalledTimes(1)
    const text = wrapper.text()
    expect(text).toContain('每日备份')
    expect(text).toContain('备份配置')
    expect(text).toContain('0 2 * * *')
    expect(text).toContain('成功')
    // 启用中且有 nextRunAt：渲染 formatDateTime 结果；绝不出现 Invalid Date
    expect(text).toContain(formatDateTime(1780000000000))
    expect(text).not.toContain('Invalid Date')
  })

  it('新建任务提交 camelCase 载荷并刷新列表', async () => {
    createTaskMock.mockResolvedValue({ ...TASK })
    const wrapper = await mountPage()

    await (await findButton(wrapper, '新建任务')).trigger('click')
    await settle()

    const nameInput = wrapper.find('input[placeholder="请输入任务名称"]')
    expect(nameInput.exists()).toBe(true)
    await nameInput.setValue('每日备份')
    // 读取类任务必须填目标 ID：前端校验会拦截空值，这里按正常操作补上
    const targetInput = wrapper.find('input[placeholder="要读取的点位 ID，如 temp_01"]')
    expect(targetInput.exists()).toBe(true)
    await targetInput.setValue('temp_01')

    await (await findButton(wrapper, '创建')).trigger('click')
    await settle()

    // 表单默认值：type=read_point、cron='*/5 * * * *'、enabled=true
    expect(createTaskMock).toHaveBeenCalledTimes(1)
    expect(createTaskMock).toHaveBeenCalledWith({
      name: '每日备份',
      type: 'read_point',
      target: 'temp_01',
      cron: '*/5 * * * *',
      enabled: true,
    })
    expect(ElMessage.success).toHaveBeenCalledWith('已创建')
    expect(listTasksMock).toHaveBeenCalledTimes(2)
  })

  it('点击立即执行调用 runTaskNow，成功提示并刷新列表', async () => {
    runTaskNowMock.mockResolvedValue({ status: 'success' })
    const wrapper = await mountPage()

    await (await findButton(wrapper, '立即执行')).trigger('click')
    await settle()

    expect(runTaskNowMock).toHaveBeenCalledWith('t1')
    expect(ElMessage.success).toHaveBeenCalledWith('执行成功')
    expect(listTasksMock).toHaveBeenCalledTimes(2)
  })

  it('立即执行结果为失败时提示错误文案', async () => {
    runTaskNowMock.mockResolvedValue({ status: 'error', error: '任务超时' })
    const wrapper = await mountPage()

    await (await findButton(wrapper, '立即执行')).trigger('click')
    await settle()

    expect(runTaskNowMock).toHaveBeenCalledWith('t1')
    expect(ElMessage.error).toHaveBeenCalledWith('执行失败：任务超时')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('关闭状态开关调用 updateTask（全量更新，enabled 取反）', async () => {
    updateTaskMock.mockResolvedValue({ ...TASK, enabled: false })
    const wrapper = await mountPage()

    const switchEl = wrapper.find('.el-switch')
    expect(switchEl.exists()).toBe(true)
    await switchEl.trigger('click')
    await settle()

    expect(updateTaskMock).toHaveBeenCalledTimes(1)
    expect(updateTaskMock).toHaveBeenCalledWith(expect.objectContaining({ id: 't1', enabled: false }))
  })

  it('确认删除后调用 deleteTask 并刷新列表', async () => {
    const wrapper = await mountPage()

    await (await findButton(wrapper, '删除')).trigger('click')
    await settle()

    expect(confirmDeleteMock).toHaveBeenCalledWith('每日备份')
    expect(deleteTaskMock).toHaveBeenCalledWith('t1')
    expect(ElMessage.success).toHaveBeenCalledWith('已删除')
    expect(listTasksMock).toHaveBeenCalledTimes(2)
  })

  it('创建接口报错时提示错误且对话框保留不崩', async () => {
    createTaskMock.mockRejectedValue(new Error('boom'))
    const wrapper = await mountPage()

    await (await findButton(wrapper, '新建任务')).trigger('click')
    await settle()
    await wrapper.find('input[placeholder="请输入任务名称"]').setValue('每日备份')
    await wrapper.find('input[placeholder="要读取的点位 ID，如 temp_01"]').setValue('temp_01')
    await (await findButton(wrapper, '创建')).trigger('click')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('boom'))
    expect(ElMessage.success).not.toHaveBeenCalled()
    // 失败后对话框仍打开（showCreate 未复位），用户可修正后重试
    expect(wrapper.find('input[placeholder="请输入任务名称"]').exists()).toBe(true)
  })
})
