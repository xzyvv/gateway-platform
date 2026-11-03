import { describe, it, expect, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import GatewayScanDialog from './GatewayScanDialog.vue'

/** GatewayScanDialog 测试
 *
 * 注：el-dialog 内部用 teleport 把 body 渲染到 document.body，
 * 在 vue-test-utils 默认 mount 下不会渲染到 wrapper.html 里。
 * 这里直接断言 props 接收 + 模板分发是否正确（不依赖 DOM 命中）。
 */

const allWrappers: ReturnType<typeof mount>[] = []
afterEach(() => {
  // 清理每个测试 attach 到 document.body 的 wrapper（含 teleport 内容）
  for (const w of allWrappers) w.unmount()
  allWrappers.length = 0
  // 顺手清掉 body 残留 dialog
  document.body.innerHTML = ''
})

describe('GatewayScanDialog', () => {
  it('scanning=true 时模板条件正确', async () => {
    const wrapper = mount(GatewayScanDialog, {
      props: { modelValue: true, scanning: true, results: [] },
      attachTo: document.body,
    })
    allWrappers.push(wrapper)
    await nextTick()
    expect(document.querySelector('.scan-loading')).toBeTruthy()
    expect(document.querySelector('.scan-empty')).toBeFalsy()
  })

  it('scanning=false 且 results 为空时显示空态', async () => {
    const wrapper = mount(GatewayScanDialog, {
      props: { modelValue: true, scanning: false, results: [] },
      attachTo: document.body,
    })
    allWrappers.push(wrapper)
    await nextTick()
    expect(document.querySelector('.scan-empty')).toBeTruthy()
    expect(document.querySelector('.el-table')).toBeFalsy()
  })

  it('有结果时 el-table 渲染（行数 == results 长度）', async () => {
    const results = [
      { ip: '192.168.1.100', port: 8080, name: 'gw-1', latency_ms: 5 },
      { ip: '192.168.1.101', port: 8080, name: 'gw-2', latency_ms: 8 },
    ]
    const wrapper = mount(GatewayScanDialog, {
      props: { modelValue: true, scanning: false, results },
      attachTo: document.body,
    })
    allWrappers.push(wrapper)
    await nextTick()
    const rows = document.querySelectorAll('tbody tr')
    expect(rows).toHaveLength(2)
  })

  it('点击关闭按钮 emit update:modelValue=false', async () => {
    const wrapper = mount(GatewayScanDialog, {
      props: { modelValue: true, scanning: false, results: [] },
      attachTo: document.body,
    })
    allWrappers.push(wrapper)
    await nextTick()
    const closeBtn = Array.from(document.querySelectorAll('button')).find((b) => b.textContent?.includes('关闭')) as HTMLButtonElement
    expect(closeBtn).toBeTruthy()
    closeBtn.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    await nextTick()
    expect(wrapper.emitted('update:modelValue')?.[0]?.[0]).toBe(false)
  })

  it('点击"重新扫描" emit rescan', async () => {
    const wrapper = mount(GatewayScanDialog, {
      props: { modelValue: true, scanning: false, results: [] },
      attachTo: document.body,
    })
    allWrappers.push(wrapper)
    await nextTick()
    const btn = Array.from(document.querySelectorAll('button')).find((b) => b.textContent?.includes('重新扫描')) as HTMLButtonElement
    expect(btn).toBeTruthy()
    btn.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    await nextTick()
    expect(wrapper.emitted('rescan')).toBeTruthy()
  })

  it('addDisabled=true 时 props 接收正确（按钮 disabled 由 el-button 接收 props.addDisabled）', () => {
    const wrapper = mount(GatewayScanDialog, {
      props: { modelValue: true, scanning: false, results: [], addDisabled: true },
    })
    // 注：build 走纯 tsc（.vue shim 下 props 推断为 {}），与 vue-tsc 的精确 props 类型不一致，
    // 故经 unknown 归一为 Record 后按 key 断言（两套编译器下均通过）。
    const props = wrapper.props() as unknown as Record<string, unknown>
    expect(props.addDisabled).toBe(true)
  })
})