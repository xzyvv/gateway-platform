/**
 * 现场调试向导数据：按「协议 + 症状」沉淀专家排查路径。
 *
 * 现场新人通常知道「读不到数」，但不知道先查接线还是先查从站号。
 * 这里把常见故障拆成有序检查项，每项给出判断依据与可跳转的处置入口；
 * 页面只负责渲染与勾选进度，流程内容集中在本文件，便于持续补充经验。
 *
 * 约束：`action.to` 只允许使用站内真实路由（含已有 Hub 的 `?tab=`），
 * `protocol` 只取 {@link WIZARD_PROTOCOLS} 中的值。
 */
import type { BusConfig } from '@/api/types'

export const WIZARD_PROTOCOLS = [
  'modbus-rtu',
  'modbus-tcp',
  'dlt645',
  'cjt188',
  'mqtt',
  'generic',
] as const

export type WizardProtocol = (typeof WIZARD_PROTOCOLS)[number]

/** 协议展示名，与筛选按钮和提示文案共用 */
export const PROTOCOL_LABELS: Record<WizardProtocol, string> = {
  'modbus-rtu': 'Modbus RTU',
  'modbus-tcp': 'Modbus TCP',
  'dlt645': 'DLT645',
  'cjt188': 'CJT188',
  'mqtt': 'MQTT 上行',
  'generic': '通用',
}

export interface WizardStep {
  id: string
  /** 检查项标题，动宾结构 */
  title: string
  /** 为什么查、查到什么算正常 */
  detail: string
  /** 可跳转的处置入口（站内路由） */
  action?: { label: string; to: string }
  /** 自动检查项：交给页面执行现成检测 */
  probe?: 'bus' | 'health' | 'device'
}

export interface WizardFlow {
  id: string
  protocol: WizardProtocol
  /** 用户视角的症状，如「读不到数」「CRC 频繁失败」 */
  symptom: string
  intro: string
  steps: WizardStep[]
}

export const debugWizardFlows: WizardFlow[] = [
  {
    id: 'modbus-rtu-read-empty',
    protocol: 'modbus-rtu',
    symptom: '读不到数 / 从站无应答',
    intro:
      'Modbus RTU 从站完全无响应时，按「物理层 → 通信参数 → 从站号 → 地址」顺序排查；每一步都有明确判据，不要同时改多个参数。',
    steps: [
      {
        id: 'confirm-rtu-bus',
        title: '确认当前调试总线是 Modbus RTU',
        detail:
          '调试台选中的总线必须为串口 RTU（协议 modbus 且未配置 tcp_addr）。若填了 TCP 地址，实际走的是 Modbus TCP，排查路径不同。',
        probe: 'bus',
      },
      {
        id: 'check-ab-wiring',
        title: '检查 A/B 接线与终端电阻',
        detail:
          'RS485 的 A/B 接反是最常见原因，可交换 A、B 再试一次；总线两端各接 120Ω 终端电阻，屏蔽层单点接地。正常收发时设备 TX/RX 指示灯应闪烁。',
        action: { label: '报文捕获看原始帧', to: '/acquisition/console?tab=packet-capture' },
      },
      {
        id: 'match-serial-params',
        title: '核对串口参数与设备铭牌',
        detail:
          '波特率、数据位、校验位、停止位必须与设备铭牌/手册一致（常见 9600 8N1）；现场拨码开关也要一致。三处不一致时表现为完全无应答。',
        action: { label: '通信参数', to: '/acquisition/comm-tuning?tab=comm-params' },
      },
      {
        id: 'check-slave-id',
        title: '核对从站号并扫描该地址',
        detail:
          '从站号取值 1–247，必须与设备拨码/铭牌一致，且同一总线不能重复。用从站扫描确认该地址是否真的有响应，避免把「地址写错」当成「设备坏」。',
        action: { label: '从站扫描', to: '/acquisition/console?tab=slave-scan' },
      },
      {
        id: 'check-bus-health',
        title: '查看健康报告中的总线异常',
        detail:
          '健康报告会汇总总线全离线、采集失败率、超时率等问题。若报告已指出总线名或错误码，优先按该问题处置。',
        probe: 'health',
      },
    ],
  },
  {
    id: 'modbus-rtu-crc-fail',
    protocol: 'modbus-rtu',
    symptom: 'CRC 校验频繁失败',
    intro:
      '偶发 CRC 失败多为干扰或接线问题，持续失败则先怀疑参数与终端匹配。按「干扰 → 终端电阻 → 参数 → 报文核对」逐项排除。',
    steps: [
      {
        id: 'confirm-rtu-params',
        title: '确认 RTU 参数与设备完全一致',
        detail:
          '波特率、校验位、停止位任一不一致都会让 CRC 大面积失败。先确认总线配置与设备铭牌一致，再动接线。',
        action: { label: '通信参数', to: '/acquisition/comm-tuning?tab=comm-params' },
        probe: 'bus',
      },
      {
        id: 'check-shield-ground',
        title: '检查屏蔽双绞线与接地',
        detail:
          '使用 485 专用屏蔽双绞线，屏蔽层接保护地；远离变频器、电机动力线与强电桥架。共模干扰会造成 CRC 随机失败。',
        action: { label: '报文捕获看原始帧', to: '/acquisition/console?tab=packet-capture' },
      },
      {
        id: 'add-termination',
        title: '接入 120Ω 终端电阻',
        detail:
          '长距离或高波特率时必须在总线两端做终端匹配，中间分支尽量短。缺少终端电阻会出现信号反射，表现为 CRC 错误或帧错位。',
      },
      {
        id: 'lower-baud-retry',
        title: '降低波特率并调整重试',
        detail:
          '把 9600 降到 4800/2400 试验，同时把超时设为 1000–1500ms、重试 2–3 次观察 CRC 是否下降。若明显好转，说明是线路质量或干扰问题。',
        action: { label: '通信参数', to: '/acquisition/comm-tuning?tab=comm-params' },
      },
      {
        id: 'compare-captured-frame',
        title: '对照报文捕获确认帧完整性',
        detail:
          '在报文捕获里看请求帧与响应帧是否完整、CRC 是否可复算；若响应帧本身就残缺，继续查线路，不要改点位配置。',
        probe: 'health',
      },
    ],
  },
  {
    id: 'modbus-tcp-unreachable',
    protocol: 'modbus-tcp',
    symptom: '连接被拒 / 连接超时',
    intro:
      'Modbus TCP 排查先确认「网络可达」，再确认「单元号正确」，最后才调并发与超时。网络层没通时改任何协议参数都无效。',
    steps: [
      {
        id: 'confirm-tcp-bus',
        title: '确认调试目标是 Modbus TCP 总线',
        detail:
          '总线需配置 tcp_addr（host:port），且协议为 modbus。若误选了串口总线，报文会发到本地串口而不是设备。',
        probe: 'bus',
      },
      {
        id: 'test-ip-port',
        title: '测试目标 IP 与 502 端口连通',
        detail:
          '先 ping 目标 IP，确认网关与设备同网段、网线已接好；再测 502 端口是否开放，检查防火墙与端口占用。',
        action: { label: '连接测试', to: '/ops?tab=connection-test' },
      },
      {
        id: 'check-unit-id',
        title: '核对从站号（Unit ID）',
        detail:
          'Modbus TCP 网关下多台表通常使用不同 Unit ID，直连 PLC 常用 1 或 255。Unit ID 与设备文档不一致时，TCP 能连上但读写无响应。',
        action: { label: '从站扫描', to: '/acquisition/console?tab=slave-scan' },
      },
      {
        id: 'tune-tcp-concurrency',
        title: '降低 TCP 并发并调整超时',
        detail:
          '同一 TCP 网关并发过高可能被拒绝或丢包。把 tcp_concurrent 降到 1–4、超时设为 1000–3000ms，重连退避保留默认 1/2/5/10/30 秒。',
        action: { label: '运行参数', to: '/system/config?tab=runtime-config' },
      },
      {
        id: 'check-tcp-health',
        title: '查看健康报告中的连接异常',
        detail:
          '健康报告会列出总线全离线、重连次数与最近错误码；结合错误码判断是网络不可达、连接被拒还是对端重置。',
        probe: 'health',
      },
    ],
  },
  {
    id: 'dlt645-no-data',
    protocol: 'dlt645',
    symptom: '读不到电表数据',
    intro:
      'DLT645 的关键差异点是「表地址」与「协议版本」，这两项写错时表现为完全无应答或校验失败。先核对铭牌，再核对参数。',
    steps: [
      {
        id: 'check-meter-address',
        title: '核对电表地址与铭牌',
        detail:
          'DLT645 表地址为 12 位 BCD（6 字节），必须与电表铭牌/资产编号一致；广播地址全 AA 只用于校时等广播命令。1997 与 2007 的地址字节序不同。',
        action: { label: '设备清单', to: '/devices?tab=devices' },
      },
      {
        id: 'check-dlt-version',
        title: '核对协议版本与地址变体',
        detail:
          '总线协议需为 dlt645，variant 选 2007（默认）或 1997 及对应地址序/是否加密的子变体，必须与电表年代和厂家说明一致。',
        action: { label: '运行参数', to: '/system/config?tab=runtime-config' },
        probe: 'bus',
      },
      {
        id: 'match-meter-serial',
        title: '核对串口参数',
        detail:
          '常见 2400/9600 波特率、偶校验 even、8 数据位、1 停止位，最终以电表铭牌为准。校验位或停止位不一致时整帧都收不到。',
        action: { label: '通信参数', to: '/acquisition/comm-tuning?tab=comm-params' },
      },
      {
        id: 'check-meter-power',
        title: '检查 485 接线与表计供电',
        detail:
          'A/B 接反、表计未上电、总线挂表过多都会导致无应答。先单表直连验证，确认单表可读后再逐台接入总线。',
        action: { label: '报文捕获看原始帧', to: '/acquisition/console?tab=packet-capture' },
      },
      {
        id: 'send-read-frame',
        title: '手工发送一帧读地址命令',
        detail:
          '用报文编辑发送一帧已知 DI（如电压 00010000）的读取命令，确认是否有回帧。有回帧说明链路正常，问题在点位地址或解析。',
        action: { label: '报文编辑', to: '/acquisition/console?tab=packet-editor' },
      },
      {
        id: 'check-dlt-health',
        title: '查看健康报告中的总线异常',
        detail:
          '若报告给出总线全离线或超时率偏高，优先按报告中的错误码核对通信参数与接线，而不是继续改点位。',
        probe: 'health',
      },
    ],
  },
  {
    id: 'cjt188-no-response',
    protocol: 'cjt188',
    symptom: '表计无应答',
    intro:
      'CJT188 的表号与方言（2004/2018）是两个独立开关，任一不一致都会无应答。先确认表号，再确认方言和串口参数。',
    steps: [
      {
        id: 'check-cjt-meter-id',
        title: '核对表号与设备地址',
        detail:
          'CJ/T188 表号范围 1–99999999（8 位 BCD），必须与铭牌一致，同总线不可重复。表号写错时设备不会回帧。',
        action: { label: '设备清单', to: '/devices?tab=devices' },
      },
      {
        id: 'check-cjt-variant',
        title: '选择正确的协议方言',
        detail:
          'CJT188 的 variant 需在 2018（默认内建）与 2004（标准方言）之间选择，必须与表计检定年份/厂家说明一致；方言不一致常表现为校验失败或无声。',
        action: { label: '运行参数', to: '/system/config?tab=runtime-config' },
        probe: 'bus',
      },
      {
        id: 'match-cjt-serial',
        title: '核对串口参数',
        detail:
          '常见 9600 8E1，部分热量表为 2400/1200；波特率、校验位、停止位以铭牌为准，不能套用 Modbus 的默认值。',
        action: { label: '通信参数', to: '/acquisition/comm-tuning?tab=comm-params' },
      },
      {
        id: 'check-cjt-wiring',
        title: '检查接线、供电与总线负载',
        detail:
          '先单表直连，排除 A/B 接反与表计未上电；多表时检查 485 驱动能力与供电，挂表过多时加中继器或分总线。',
      },
      {
        id: 'check-cjt-health',
        title: '查看健康报告中的总线异常',
        detail:
          '健康报告的总线明细会给出成功率与最近错误码；结合错误码区分「无应答」与「校验失败」，再决定查线还是改参数。',
        probe: 'health',
      },
    ],
  },
  {
    id: 'mqtt-uplink-ineffective',
    protocol: 'mqtt',
    symptom: 'MQTT 上行不生效',
    intro:
      '上行不生效先区分「MQTT 没连上」与「连上了但没发/发错」。按连接状态 → Broker 参数 → 转发通道 → 实测发送 → 队列顺序排查。',
    steps: [
      {
        id: 'check-mqtt-connection',
        title: '检查 MQTT 连接状态',
        detail:
          '先看上行状态页的连接数与错误信息；健康报告中的 MQTT_DISCONNECTED 等问题会直接指出断链原因。',
        action: { label: '上行状态', to: '/uplink?tab=north-status' },
        probe: 'health',
      },
      {
        id: 'check-broker-config',
        title: '核对 Broker 地址、认证与 ClientID',
        detail:
          '地址与端口（1883/8883）、用户名密码、ClientID 必须与 Broker 一致；TLS 使用 8883 并确保 CA 证书有效。ClientID 重复会被 Broker 踢下线。',
        action: { label: '运行参数', to: '/system/config?tab=runtime-config' },
      },
      {
        id: 'check-forward-channel',
        title: '确认转发通道已启用',
        detail:
          '上行配置中通道 enabled 必须为 true，目标 topic 与 QoS 正确；模板编码（JSON/自定义）要与接收端解析方式一致。',
        action: { label: '上行配置', to: '/uplink?tab=forward-config' },
      },
      {
        id: 'test-uplink-payload',
        title: '用上行测试验证渲染与发送',
        detail:
          '在上行测试页用样例数据先「仅渲染」检查报文内容，再执行真实上行并查看 Broker 是否收到；这一步可区分「模板错」与「网络错」。',
        action: { label: '上行测试', to: '/uplink?tab=uplink-tester' },
      },
      {
        id: 'check-queue-backlog',
        title: '查看队列积压与断连续传',
        detail:
          '若连接正常但数据延迟，检查队列存量是否持续增长；断网期间的积压会在恢复后按策略补发，积压过大需先处理网络或降低上报频率。',
        action: { label: '断连续传', to: '/data?tab=data-buffering' },
      },
    ],
  },
  {
    id: 'point-write-readback-mismatch',
    protocol: 'generic',
    symptom: '点位写入失败 / 回读不一致',
    intro:
      '写入问题先确认「允许写」，再确认「地址与功能码」，最后核对数据类型与倍率。写成功但回读不一致，多半是地址偏移或字节序。',
    steps: [
      {
        id: 'check-target-device',
        title: '确认目标设备在线',
        detail:
          '先确认调试设备已选中且在线；设备离线时写入会直接失败，先回到链路排查，不要改点位配置。',
        probe: 'device',
      },
      {
        id: 'check-write-whitelist',
        title: '确认点位已开启允许写入',
        detail:
          '点位需显式开启「允许写入」，否则后端会以 not in whitelist 拒绝。批量写入前先确认目标点位都在白名单内。',
        action: { label: '点位清单', to: '/points?tab=points' },
      },
      {
        id: 'check-register-func',
        title: '核对寄存器地址与功能码',
        detail:
          '确认功能码（05/06/15/16）与寄存器类型匹配，并注意 0-based 与 1-based 偏移；PLC 手册地址通常需要 ±1 换算。',
        action: { label: '地址换算', to: '/points?tab=addr-calculator' },
      },
      {
        id: 'check-data-type-scale',
        title: '核对数据类型、字节序与倍率',
        detail:
          '确认 16/32 位、大小端与缩放系数；写入原始值 = 工程值 × 倍率。字节序错时回读值会出现明显倍数偏差或高低字颠倒。',
        action: { label: '通信参数', to: '/acquisition/comm-tuning?tab=comm-params' },
      },
      {
        id: 'write-then-readback',
        title: '下发后用远程控制回读',
        detail:
          '在远程控制页对同一点位写入并立即回读；若写入成功但回读不同，回到地址/字节序检查。若写入被拒，查看下行执行历史中的错误码。',
        action: { label: '远程控制', to: '/devices?tab=remote-control' },
      },
    ],
  },
]

/**
 * 总线实际协议名：BusConfig.protocol 只区分 modbus/dlt645/cjt188 等，
 * Modbus 的 RTU/TCP 由是否配置 tcp_addr 决定。展示与匹配都需要这一步归一化。
 */
export function busProtocolName(bus: Pick<BusConfig, 'protocol' | 'tcp_addr'>): string {
  const protocol = (bus.protocol || '').toLowerCase()
  if (protocol === 'modbus') return bus.tcp_addr ? 'modbus-tcp' : 'modbus-rtu'
  return protocol
}

/** 当前总线是否与排查流程的协议匹配（兼容 protocol=modbus + tcp_addr 的写法） */
export function busMatchesProtocol(
  bus: Pick<BusConfig, 'protocol' | 'tcp_addr'>,
  protocol: WizardProtocol,
): boolean {
  const actual = busProtocolName(bus)
  switch (protocol) {
    case 'modbus-rtu':
      return actual === 'modbus-rtu' || actual === 'modbus' || actual === 'rtu'
    case 'modbus-tcp':
      return actual === 'modbus-tcp' || actual.includes('tcp')
    case 'dlt645':
      return actual === 'dlt645'
    case 'cjt188':
      return actual === 'cjt188'
    case 'mqtt':
      return actual === 'mqtt'
    case 'generic':
      return true
    default:
      return false
  }
}
