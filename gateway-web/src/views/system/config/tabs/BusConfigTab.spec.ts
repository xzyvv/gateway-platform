import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElFormItem, ElSelect } from 'element-plus'
import EmptyState from '@/components/EmptyState.vue'

/**
 * D1：参数配置 › 总线配置页签行为测试。
 *
 * 这一页决定「采集器连什么、怎么连」，字段选错等于现场从第一天就采不到数。
 * 必须钉住的几条：
 *   1. 串行协议才出现波特率/校验/数据位/停止位；网络协议必须出现对端地址；
 *      Modbus 例外——它是串行协议但填了 tcp_addr 就走 TCP，所以两者都显示；
 *   2. OPC UA 认证默认匿名 + 明文，且**不在渲染期写回模型**（不配置即保持 null）；
 *   3. 安全策略与安全模式互为约束，选择时自动纠正，别等保存才被后端拦下；
 *   4. 空列表要有创建入口，删除要按索引上抛。
 */

vi.mock('@/composables/devices/useProtocol', () => ({
  protocolLabel: {
    modbus: 'Modbus RTU/TCP',
    dlt645: 'DL/T 645',
    iec104: 'IEC 104',
    opcua: 'OPC UA',
    iec101: 'IEC 101',
  } as Record<string, string>,
  protocolType: () => 'primary',
  protocolTransportOf: (p: string) =>
    ({ iec104: 'tcp', opcua: 'tcp', modbus: 'serial', dlt645: 'serial', iec101: 'serial' })[p] ?? 'serial',
  protocolVariantsOf: (p: string) => (p === 'iec101' ? [{ value: 'iec101_balanced', label: '平衡式' }] : []),
  protocolEndpointPlaceholder: (p: string) => `对端地址（${p}）`,
}))

import BusConfigTab from './BusConfigTab.vue'

interface BusLike {
  port_name: string
  protocol: string
  baud_rate?: number
  parity?: string
  data_bits?: number
  stop_bits?: number
  tcp_addr?: string
  variant?: string | null
  opcua?: Record<string, unknown>
  asdu_addr_len?: number | null
  tcp_concurrent?: number | null
}

function bus(over: Partial<BusLike> = {}): BusLike {
  return {
    port_name: 'COM3',
    protocol: 'modbus',
    baud_rate: 9600,
    parity: 'none',
    data_bits: 8,
    stop_bits: 1,
    tcp_addr: '',
    variant: null,
    ...over,
  }
}

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountTab(buses: BusLike[], portErrors = new Map<number, string>()) {
  return mount(BusConfigTab, {
    props: { modelValue: buses as never, portErrors },
    global: {
      stubs: {
        PortSelect: true,
        BaudrateSelect: true,
        ParitySelect: true,
        DataBitsSelect: true,
        StopBitsSelect: true,
        ProtocolSelector: true,
      },
    },
  })
}

function labels(wrapper: ReturnType<typeof mountTab>): string[] {
  return wrapper.findAll('.el-form-item__label').map((l) => l.text())
}

function selects(wrapper: ReturnType<typeof mountTab>) {
  return wrapper.findAllComponents(ElSelect)
}

describe('BusConfigTab 总线配置', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('串行协议：显示波特率/校验位/数据位/停止位，总线名走串口选择', async () => {
    const buses = [bus({ protocol: 'dlt645' })]
    const wrapper = mountTab(buses)

    const text = labels(wrapper)
    expect(text).toContain('波特率')
    expect(text).toContain('校验位')
    expect(text).toContain('数据位')
    expect(text).toContain('停止位')
    expect(wrapper.findAllComponents({ name: 'PortSelect' })).toHaveLength(1)
    // dlt645 无对端地址字段
    expect(text).not.toContain('对端地址')
    expect(wrapper.text()).toContain('串行')
  })

  it('网络协议：不显示串行参数，必须出现对端地址与「必填」提示', async () => {
    const wrapper = mountTab([bus({ protocol: 'iec104', port_name: 'iec104_1' })])

    const text = labels(wrapper)
    expect(text).not.toContain('波特率')
    expect(text).toContain('对端地址')
    expect(wrapper.text()).toContain('网络协议必填')
    expect(wrapper.text()).toContain('网络')
    // 网络协议的总线名是纯标识输入框
    expect(wrapper.find('input[placeholder="如: iec104_1（仅作总线标识）"]').exists()).toBe(true)
  })

  it('Modbus 是对端地址的例外：填了才走 TCP，提示文案必须说清', async () => {
    const wrapper = mountTab([bus()])

    const text = labels(wrapper)
    expect(text).toContain('对端地址')
    expect(text).toContain('波特率')
    expect(wrapper.text()).toContain('留空走串口 Modbus RTU / ASCII；填写后走 Modbus TCP')
  })

  it('切换协议只上抛事件，不擅自改模型', async () => {
    const buses = [bus()]
    const wrapper = mountTab(buses)

    wrapper.findAllComponents({ name: 'ProtocolSelector' })[0]!.vm.$emit('update:modelValue', 'iec104')
    await settle()

    const emitted = wrapper.emitted('protocol-change') as unknown as Array<[BusLike, string]>
    expect(emitted).toHaveLength(1)
    expect(emitted[0]![1]).toBe('iec104')
    expect(buses[0]!.protocol).toBe('modbus')
  })

  it('协议变体只在协议有变体时出现，选择后写入 variant，清空写回 null', async () => {
    const buses = [bus({ protocol: 'iec101', variant: null })]
    const wrapper = mountTab(buses)

    const variantSelect = selects(wrapper)[0]!
    variantSelect.vm.$emit('update:modelValue', 'iec101_balanced')
    await settle()
    expect(buses[0]!.variant).toBe('iec101_balanced')

    variantSelect.vm.$emit('update:modelValue', '')
    await settle()
    expect(buses[0]!.variant).toBeNull()

    // 没有变体的协议不渲染这一项
    const noVariant = mountTab([bus()])
    expect(labels(noVariant)).not.toContain('协议变体')
  })

  it('OPC UA 默认匿名 + 明文，渲染期不写回 opcua（不配置就是 null）', async () => {
    const buses = [bus({ protocol: 'opcua', port_name: 'opcua_1', tcp_addr: 'opc.tcp://10.0.0.9:4840' })]
    const wrapper = mountTab(buses)

    expect(wrapper.text()).toContain('OPC UA 认证（不配置即匿名只读）')
    const sel = selects(wrapper)
    expect(sel[0]!.props('modelValue')).toBe('anonymous')
    expect(sel[1]!.props('modelValue')).toBe('none')
    expect(sel[2]!.props('modelValue')).toBe('none')
    // 关键：只读展示不得物化出 opcua 对象
    expect(buses[0]!.opcua).toBeUndefined()
  })

  it('切到用户名认证：出现用户名/密码输入，并把值写回 opcua', async () => {
    const buses = [bus({ protocol: 'opcua', tcp_addr: 'opc.tcp://10.0.0.9:4840' })]
    const wrapper = mountTab(buses)

    selects(wrapper)[0]!.vm.$emit('update:modelValue', 'username')
    await settle()

    expect(wrapper.find('input[placeholder="Server 分配的用户名"]').exists()).toBe(true)
    expect(wrapper.find('input[placeholder="支持 enc: 密文"]').exists()).toBe(true)

    await wrapper.find('input[placeholder="Server 分配的用户名"]').setValue('ops')
    await settle()
    expect(buses[0]!.opcua).toMatchObject({ auth: 'username', username: 'ops', security_mode: 'none' })
  })

  it('安全策略切到非 none 时，安全模式自动升级为签名加密', async () => {
    const buses = [bus({ protocol: 'opcua', tcp_addr: 'opc.tcp://10.0.0.9:4840' })]
    const wrapper = mountTab(buses)

    selects(wrapper)[1]!.vm.$emit('update:modelValue', 'basic256sha256')
    await settle()

    expect(buses[0]!.opcua).toMatchObject({
      security_policy: 'basic256sha256',
      security_mode: 'sign_and_encrypt',
    })
  })

  it('安全策略切回 none 时，安全模式被强制回退为 none', async () => {
    const buses = [
      bus({
        protocol: 'opcua',
        tcp_addr: 'opc.tcp://10.0.0.9:4840',
        opcua: { auth: 'anonymous', security_policy: 'basic256sha256', security_mode: 'sign' },
      }),
    ]
    const wrapper = mountTab(buses)

    selects(wrapper)[1]!.vm.$emit('update:modelValue', 'none')
    await settle()

    expect(buses[0]!.opcua).toMatchObject({ security_policy: 'none', security_mode: 'none' })
  })

  it('选了签名模式但策略仍是 none 时，策略自动升到 Basic256Sha256', async () => {
    const buses = [bus({ protocol: 'opcua', tcp_addr: 'opc.tcp://10.0.0.9:4840' })]
    const wrapper = mountTab(buses)

    selects(wrapper)[2]!.vm.$emit('update:modelValue', 'sign')
    await settle()

    expect(buses[0]!.opcua).toMatchObject({ security_mode: 'sign', security_policy: 'basic256sha256' })
  })

  it('X.509 认证或非 none 策略才要求证书与私钥路径', async () => {
    const buses = [bus({ protocol: 'opcua', tcp_addr: 'opc.tcp://10.0.0.9:4840' })]
    const wrapper = mountTab(buses)

    expect(wrapper.find('input[placeholder="/etc/gateway/opcua/client.crt"]').exists()).toBe(false)
    selects(wrapper)[0]!.vm.$emit('update:modelValue', 'certificate')
    await settle()
    expect(wrapper.find('input[placeholder="/etc/gateway/opcua/client.crt"]').exists()).toBe(true)
    expect(wrapper.find('input[placeholder="/etc/gateway/opcua/client.key"]').exists()).toBe(true)
  })

  it('IEC104 才有公共地址长度，清空写回 null（默认 2 字节）', async () => {
    const buses = [bus({ protocol: 'iec104', asdu_addr_len: null })]
    const wrapper = mountTab(buses)

    expect(labels(wrapper)).toContain('公共地址长度')
    selects(wrapper)[0]!.vm.$emit('update:modelValue', 1)
    await settle()
    expect(buses[0]!.asdu_addr_len).toBe(1)

    selects(wrapper)[0]!.vm.$emit('update:modelValue', undefined)
    await settle()
    expect(buses[0]!.asdu_addr_len).toBeNull()

    expect(labels(mountTab([bus()]))).not.toContain('公共地址长度')
  })

  it('仅当填写了 tcp_addr 才出现 TCP 并发（串口总线恒为 1）', async () => {
    expect(labels(mountTab([bus()]))).not.toContain('TCP 并发')

    const wrapper = mountTab([bus({ tcp_addr: '192.168.1.10:502' })])
    expect(labels(wrapper)).toContain('TCP 并发')
    expect(wrapper.text()).toContain('单连接同时在途请求数，默认 8')
  })

  it('总线名为空时把该行的校验错误透传给表单项', async () => {
    const wrapper = mountTab([bus()], new Map([[0, '总线名称不能为空']]))

    const nameItem = wrapper.findAllComponents(ElFormItem).find((i) => i.props('label') === '总线名称')!
    expect(nameItem.props('error')).toBe('总线名称不能为空')
  })

  it('删除按索引上抛，新增走 add 事件', async () => {
    const wrapper = mountTab([bus(), bus({ port_name: 'COM4' })])

    const deleteBtns = wrapper.findAll('button').filter((b) => b.text() === '删除')
    expect(deleteBtns).toHaveLength(2)
    await deleteBtns[1]!.trigger('click')
    expect(wrapper.emitted('remove')).toEqual([[1]])

    const addBtns = wrapper.findAll('button').filter((b) => b.text().includes('新增总线'))
    await addBtns[0]!.trigger('click')
    expect(wrapper.emitted('add')).toHaveLength(1)
  })

  it('空列表：显示空态，空态按钮同样触发新增', async () => {
    const wrapper = mountTab([])

    const empty = wrapper.findComponent(EmptyState)
    expect(empty.exists()).toBe(true)
    expect(empty.text()).toContain('还没有总线')
    empty.vm.$emit('action')
    await settle()
    expect(wrapper.emitted('add')).toHaveLength(1)
  })
})
