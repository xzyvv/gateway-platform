import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

const { routeQuery } = vi.hoisted(() => ({ routeQuery: {} as Record<string, string> }))

vi.mock('vue-router', () => ({ useRoute: () => ({ query: routeQuery }) }))
vi.mock('@/api/protocolTools', () => ({
  listPresets: vi.fn(),
  sendFrame: vi.fn(),
}))
vi.mock('@/composables/acquisition/useDebugTargets', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  return {
    useDebugTargets: () => ({
      buses: ref([{ port_name: '/dev/ttyUSB0', protocol: 'modbus' }]),
      busOptions: ref([{ label: '调试总线', value: '/dev/ttyUSB0' }]),
      loading: ref(false),
      selectedBus: ref('/dev/ttyUSB0'),
    }),
  }
})
// BusPauseBanner 在总线忙时会查暂停状态，不能让它真的发 HTTP
vi.mock('@/api/buses', () => ({
  listPausedBuses: vi.fn().mockResolvedValue({ paused_buses: [] }),
  pauseBusCollect: vi.fn(),
  resumeBusCollect: vi.fn(),
}))

import PacketEditor from './PacketEditor.vue'
import HexInput from '@/components/industrial/HexInput.vue'
import { listPresets, sendFrame, type SendResult } from '@/api/protocolTools'

/**
 * 报文编辑：这是唯一把「用户手写的帧」直接打到真实总线的入口。
 * 这里钉住三类现场事故：
 *   1. 非法 HEX 不能悄悄变成“发送上一帧”或直接下发到总线；
 *   2. 失败必须如实显示后端错误（超时/总线忙），不能伪装成通信成功；
 *   3. 离线解析字段必须真的展示出来，现场才有依据核对回帧。
 */

const wrappers: Array<ReturnType<typeof mount>> = []

async function settle() {
  await flushPromises()
  await nextTick()
}

const okResult: SendResult = {
  requestHex: '01 03 00 00 00 02',
  responseHex: '01 03 04 00 2A 00 00',
  success: true,
  latencyMs: 12,
  parsed: '读 2 个寄存器 = 42',
  error: null,
}

async function mountPage() {
  vi.mocked(listPresets).mockResolvedValue([{ name: '读保持寄存器', hex: '01 03 00 00 00 02' }])
  const wrapper = mount(PacketEditor)
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

function button(wrapper: ReturnType<typeof mount>, text: string) {
  const hit = wrapper.findAll('button').find((b) => b.text() === text)
  expect(hit, `找不到按钮「${text}」`).toBeTruthy()
  return hit!
}

describe('PacketEditor 报文编辑', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    localStorage.clear()
    delete routeQuery.hex
    vi.mocked(sendFrame).mockResolvedValue(okResult)
  })

  afterEach(() => {
    for (const w of wrappers) w.unmount()
    wrappers.length = 0
    document.body.innerHTML = ''
  })

  it('HexInput 拦截非法字符：非法内容不会进入待发送帧，发出去的仍是上一合法帧', async () => {
    const wrapper = await mountPage()

    // 用户把 "ZZ" 敲进输入框（奇数长度且含非十六进制字符）
    await wrapper.findComponent(HexInput).find('input').setValue('ZZ')
    await settle()
    // 展示层给红框提示，但不把脏数据写回 v-model
    expect((wrapper.findComponent(HexInput).find('input').element as HTMLInputElement).value).toBe('ZZ')

    await button(wrapper, '发送').trigger('click')
    await settle()

    // 防现场事故：绝不能把 "ZZ" 这种非法帧打到总线上，也不能悄悄改发上一帧
    // （那会让现场以为发的是屏幕上这段，排查方向全错）
    expect(vi.mocked(sendFrame)).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('已禁止下发')
  })

  it('合法 HEX 发送后把请求/响应/解析/耗时渲染到结果区', async () => {
    const wrapper = await mountPage()

    await button(wrapper, '发送').trigger('click')
    await settle()

    expect(vi.mocked(sendFrame)).toHaveBeenCalledWith('01 03 00 00 00 02', '/dev/ttyUSB0', 1500)
    const result = wrapper.find('.result').text()
    expect(result).toContain('01 03 00 00 00 02')
    expect(result).toContain('01 03 04 00 2A 00 00')
    expect(result).toContain('读 2 个寄存器 = 42')
    expect(result).toContain('12 ms')
    expect(result).toContain('通信成功')
    // 在线回帧也走同一解析内核，逐字段展示
    expect(wrapper.find('.online-parse').text()).toContain('响应帧解析')
  })

  it('超时失败时展示「超时无响应」与后端解析文案，不显示成功样式', async () => {
    vi.mocked(sendFrame).mockResolvedValue({
      requestHex: '01 03 00 00 00 02',
      responseHex: '',
      success: false,
      latencyMs: 1500,
      parsed: '',
      error: 'timeout',
    })
    const wrapper = await mountPage()

    await button(wrapper, '发送').trigger('click')
    await settle()

    const result = wrapper.find('.result')
    expect(result.text()).toContain('通信失败：超时无响应')
    expect(result.text()).toContain('超时，未收到响应帧')
    expect(result.text()).not.toContain('通信成功')
    // 响应帧用错误色，避免把失败回帧当正常数据处理
    expect(result.find('code.err').exists()).toBe(true)
    expect(result.find('.el-tag').classes()).toContain('el-tag--danger')
  })

  it('总线被采集占用时给出暂停采集入口，而不是只丢一句失败提示', async () => {
    vi.mocked(sendFrame).mockResolvedValue({
      requestHex: '01 03 00 00 00 02',
      responseHex: '',
      success: false,
      latencyMs: 0,
      parsed: '总线正被采集任务占用',
      error: 'bus_busy',
    })
    const wrapper = await mountPage()

    await button(wrapper, '发送').trigger('click')
    await settle()

    expect(wrapper.find('.result').text()).toContain('通信失败：总线被占用')
    expect(wrapper.find('.bus-pause-banner').text()).toContain('「/dev/ttyUSB0」正被采集引擎占用')
    expect(button(wrapper, '暂停该总线采集')).toBeTruthy()
  })

  it('离线粘贴原始帧后展示逐字段解析结果', async () => {
    const wrapper = await mountPage()

    await wrapper.find('textarea[aria-label="HEX 帧数据"]').setValue('01 03 04 00 2A 00 00')
    await button(wrapper, '解析').trigger('click')
    await settle()

    const parsed = wrapper.find('.parse-result')
    expect(parsed.exists()).toBe(true)
    const text = parsed.text()
    expect(text).toContain('从站地址')
    expect(text).toContain('功能码')
    expect(text).toContain('字节数')
    expect(text).toContain('CRC 校验')
  })

  it('离线输入含非法字符时给出中文提示而不是抛异常/静默', async () => {
    const wrapper = await mountPage()

    await wrapper.find('textarea[aria-label="HEX 帧数据"]').setValue('ZZ')
    await button(wrapper, '解析').trigger('click')
    await settle()

    expect(wrapper.find('.parse-result').text()).toContain('请输入合法的 HEX 字符')
  })

  it('输入为空时发送按钮禁用，不会发起空帧请求', async () => {
    const wrapper = await mountPage()

    wrapper.findComponent(HexInput).vm.$emit('update:modelValue', '')
    await settle()

    const send = button(wrapper, '发送')
    expect(send.attributes('disabled')).toBeDefined()
    await send.trigger('click')
    await settle()
    expect(vi.mocked(sendFrame)).not.toHaveBeenCalled()
  })

  it('URL 预填的非法 HEX 被本地拦下，不会打到总线', async () => {
    // 抓包页跳转 ?hex=... 直接写 v-model，绕开 HexInput 的字符过滤；
    // 必须在 send() 之前用 isValidHex 兜住，否则一条脏链接就能往真实总线发垃圾帧
    routeQuery.hex = 'ZZ'
    const wrapper = await mountPage()
    expect(wrapper.findComponent(HexInput).props('modelValue')).toBe('ZZ')
    expect(wrapper.text()).toContain('已禁止下发')

    await button(wrapper, '发送').trigger('click')
    await settle()

    expect(vi.mocked(sendFrame)).not.toHaveBeenCalled()
  })
})
