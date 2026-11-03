/**
 * 协议异常码知识库（DLT645 / CJT188）
 *
 * 背景：Modbus 异常码由后端静态知识库接口提供（GET /api/diagnostics/modbus-errors），
 * 但 DL/T 645 与 CJ/T 188 两种表计协议没有对应接口——现场排查时只能对着
 * 一帧 hex 猜「ERR 字节到底什么意思」。本模块把两者的错误信息字定义
 * 落到前端，供「异常码速查」页与帧解析器共用。
 *
 * 两者的错误表达机制与 Modbus 完全不同，必须分开建模：
 *
 * 1. DL/T 645-2007 —— 错误信息字 ERR 是**位标志**（bitmask），不是字节码表。
 *    从站异常应答帧：68 A0..A5 68 Cx 01 ERR CS 16，其中控制码 D6 位=1 表示异常应答，
 *    数据域长度 L=01H，数据域唯一字节即为 ERR（未加 33H 加密）。
 *    ERR 各 bit 独立置位可同时表示多个错误；未在 bit1~bit6 定义的情况
 *    统一归入 bit0「其他错误」。
 *
 * 2. CJ/T 188-2018 —— 从站异常应答帧的数据域为 `SER + 状态ST`（L=03H）。
 *    状态 ST 占 2 字节：
 *      第一字节：位标志（D0 阀门开关、D1 阀门状态、D2 电池电压 等）
 *      第二字节：诊断信息码 JD（低 4 位电池/换能器类，高 4 位温度传感器类）
 *    ST 出现在正常应答的数据域末尾（读计量数据）以及异常应答帧中。
 *
 * 注意：这里的 JD 具体码值来自主流厂商实现（汇中 SCL-61 系列），标准只
 * 规定「第二字节为厂商定义」，不同厂商取值可能不同——因此文案里标注了
 * 「厂商相关」，避免用户误以为是标准强制值。
 */

export type ProtocolErrorProtocol = 'dlt645' | 'cjt188'

export interface ProtocolErrorEntry {
  /** 码/位的展示形式，如 "ERR bit0" / "JD 0x01" */
  code: string
  /** 名称 */
  name: string
  /** 中文含义 */
  meaning: string
  /** 常见原因 */
  cause: string
  /** 建议处理方式 */
  solution: string
}

/* ============================================================
 * DL/T 645-2007 错误信息字 ERR（位标志）
 * 附录 C：Bit7 保留 / Bit6 费率数超 / Bit5 日时段数超 / Bit4 年时区数超 /
 *         Bit3 通信速率不能更改 / Bit2 密码错·未授权 / Bit1 无请求数据 / Bit0 其他错误
 * ============================================================ */

export interface Dlt645ErrBit {
  /** 位序号 0~7 */
  bit: number
  /** 位掩码 */
  mask: number
  name: string
  meaning: string
  cause: string
  solution: string
}

export const DLT645_ERR_BITS: readonly Dlt645ErrBit[] = [
  {
    bit: 0,
    mask: 0x01,
    name: '其他错误',
    meaning: '除 bit1~bit6 已定义错误之外的其他异常',
    cause: '控制码与数据标识不匹配、请求的数据域格式非法，或电表内部状态异常',
    solution: '核对请求的控制码与数据标识（DI）是否与该电表型号匹配；仍失败则记录完整报文联系厂家',
  },
  {
    bit: 1,
    mask: 0x02,
    name: '无请求数据',
    meaning: '电表内没有该数据标识对应的冻结/结算数据',
    cause: '尚未到结算日，或该数据项从未产生（如无热量表但读取热量数据标识）',
    solution: '确认是否已产生该类数据；等待结算日或改读实时量数据标识（如 DI 901FH）',
  },
  {
    bit: 2,
    mask: 0x04,
    name: '密码错 / 未授权',
    meaning: '密码校验失败或当前无该操作权限',
    cause: '密码错误 / 未做安全认证直接执行受控命令（清零、改价、改密码等）',
    solution: '核对密码与权限级别；受控命令需先完成安全认证（控制码 03H）并带上操作者代码',
  },
  {
    bit: 3,
    mask: 0x08,
    name: '通信速率不能更改',
    meaning: '电表拒绝更改通信速率',
    cause: '电表处于出厂状态 / 未通过安全认证，或该型号不支持在线改速率',
    solution: '先完成安全认证与密码授权，再执行更改速率命令；否则改用出厂默认速率通信',
  },
  {
    bit: 4,
    mask: 0x10,
    name: '年时区数超',
    meaning: '设置的夏令时区个数超出电表支持范围',
    cause: '下发的时区表数量超过电表容量（如超过 5 个时区）',
    solution: '减少时区表条目数后重试；查阅该型号手册的最大时区数',
  },
  {
    bit: 5,
    mask: 0x20,
    name: '日时段数超',
    meaning: '设置的日时段个数超出电表支持范围',
    cause: '下发的时段表数量超过电表容量（如费率时段数超限）',
    solution: '按现场实际费率削减时段表条目后重试',
  },
  {
    bit: 6,
    mask: 0x40,
    name: '费率数超',
    meaning: '设置的费率个数超出电表支持范围',
    cause: '下发的费率个数超过电表容量（常见如 4 费率表只支持 2 个费率）',
    solution: '按电表实际支持的费率数下发，多余费率项用无效值填充',
  },
]

/* ============================================================
 * CJ/T 188-2018 状态 ST
 * 第一字节：位标志（阀门开关/阀门状态/电池电压）
 * 第二字节：诊断信息码 JD（低 4 位 + 高 4 位）
 * ============================================================ */

export interface Cjt188StBit {
  bit: number
  mask: number
  name: string
  meaning: string
  cause: string
  solution: string
}

/** CJT188 状态 ST 第一字节位定义 */
export const CJT188_ST_BITS: readonly Cjt188StBit[] = [
  {
    bit: 0,
    mask: 0x01,
    name: '阀门开关状态',
    meaning: '阀门当前开合状态',
    cause: '正常反映阀门位置；异常时说明阀控机构或电机故障',
    solution: '开阀状态下仍报关阀，检查阀门是否卡滞；必要时现场手动开阀并检修电机',
  },
  {
    bit: 1,
    mask: 0x02,
    name: '阀门状态',
    meaning: '阀门状态异常标志',
    cause: '阀门处于非预期位置（半开、卡滞、电机故障）',
    solution: '检查阀门机械与电机；重新下发一次开阀/关阀命令观察是否恢复',
  },
  {
    bit: 2,
    mask: 0x04,
    name: '电池电压欠压',
    meaning: '电池电压不足',
    cause: '电池自然老化或低温导致电压下降',
    solution: '尽快更换电池；欠压下通信会不稳定甚至完全无法抄表',
  },
]

export interface Cjt188JdEntry {
  /** 诊断码（第二字节 JD） */
  code: string
  /** 数值掩码 */
  mask: number
  name: string
  meaning: string
  cause: string
  solution: string
}

/**
 * CJT188 诊断信息码 JD。
 *
 * 低 4 位（0x0f 掩码）：电池电压 / 换能器 / E2PROM 类，互斥取值。
 * 高 4 位（0x10/0x20/0x40/0x80 掩码）：温度传感器类，可与低 4 位同时置位，
 * 也可多个高 4 位同时置位（如同时超上限）。
 */
export const CJT188_JD_ENTRIES: readonly Cjt188JdEntry[] = [
  {
    code: '0x01',
    mask: 0x0f,
    name: '电池电压低于 3.37V',
    meaning: '电池电压偏低，需尽快更换',
    cause: '电池容量衰减，或长期未抄表导致深度放电',
    solution: '准备更换电池；更换前注意保存通信地址与累计量',
  },
  {
    code: '0x02',
    mask: 0x0f,
    name: '空管或换能器故障',
    meaning: '管道无水或超声换能器无测量信号',
    cause: '管道内无介质（空管），或换能器损坏、脱落',
    solution: '确认管路有水；无水属正常；始终无信号则需更换换能器或水表',
  },
  {
    code: '0x03',
    mask: 0x0f,
    name: '电池欠压 + 换能器故障',
    meaning: '低电压与无测量信号同时发生',
    cause: '电池耗尽叠加换能器异常，水表已无法正常计量',
    solution: '更换电池并检查换能器；两者需一并处理否则仍无法计量',
  },
  {
    code: '0x04',
    mask: 0x0f,
    name: '电池电压低于 3.3V',
    meaning: '电池电压严重不足，必须更换',
    cause: '电池已接近失效阈值',
    solution: '立即更换电池；该电压下水表随时可能停止响应抄表',
  },
  {
    code: '0x05',
    mask: 0x0f,
    name: '计算器与换能器通讯故障',
    meaning: '电路板与超声换能器之间无法通信',
    cause: '换能器线缆断线、接插件接触不良或电路板故障',
    solution: '检查换能器接线与接插件；线缆正常则判定电路板损坏需返修',
  },
  {
    code: '0x06',
    mask: 0x0f,
    name: 'E2PROM 损坏',
    meaning: '参数存储芯片故障',
    cause: 'EEPROM 芯片损坏或数据校验失败，累计量与参数可能已丢失',
    solution: '记录当前读数；更换水表并重新下发参数与费率',
  },
  {
    code: '0x10',
    mask: 0x10,
    name: '供水温度传感器故障',
    meaning: '供水侧温度传感器短路/开路，或温度低于 2℃',
    cause: '温度探头损坏、接线脱落，或供热不足导致水温过低',
    solution: '检查温度探头接线；低于 2℃ 属工况问题，需确认供热是否正常',
  },
  {
    code: '0x20',
    mask: 0x20,
    name: '供水温度超出 150℃',
    meaning: '供水温度超过仪表量程上限',
    cause: '供热温度过高，超出热量表设计范围',
    solution: '确认供热系统是否异常；长期超温会损坏仪表，需核查量程选型',
  },
  {
    code: '0x40',
    mask: 0x40,
    name: '回水温度传感器故障',
    meaning: '回水侧温度传感器短路/开路，或温度低于 2℃',
    cause: '温度探头损坏、接线脱落，或回水温度过低',
    solution: '检查回水温度探头接线；热量计算依赖供回水温差，回水异常会导致热量偏差',
  },
  {
    code: '0x80',
    mask: 0x80,
    name: '回水温度超出 150℃',
    meaning: '回水温度超过仪表量程上限',
    cause: '回水温度异常升高，超出仪表量程',
    solution: '核查换热与循环系统；长期超温会损坏仪表',
  },
]

/* ============================================================
 * 统一查询接口
 * ============================================================ */

/**
 * DLT645 的 ERR 字节是位标志——一个字节可同时表示多个错误。
 * 这里把置位的 bit 全部解出，供帧解析器逐条列出。
 */
export function decodeDlt645Err(errByte: number): Dlt645ErrBit[] {
  return DLT645_ERR_BITS.filter((b) => (errByte & b.mask) !== 0)
}

/**
 * CJT188 的状态 ST 是 2 字节。
 *
 * 第一字节按位解（阀门/电池），第二字节按掩码解（诊断码 JD）。
 * JD 的高 4 位可多位并存（多个温度传感器同时异常），低 4 位互斥。
 */
export function decodeCjt188St(stBytes: number[]): {
  bits: Cjt188StBit[]
  diagnostics: Cjt188JdEntry[]
} {
  const first = stBytes[0] ?? 0
  const jd = stBytes[1] ?? 0
  const bits = CJT188_ST_BITS.filter((b) => (first & b.mask) !== 0)
  const diagnostics: Cjt188JdEntry[] = []
  // 高 4 位：可并存，逐个掩码匹配（0x10/0x20/0x40/0x80 各自独立置位）
  for (const e of CJT188_JD_ENTRIES) {
    if (e.mask > 0x0f) {
      if ((jd & e.mask) === e.mask) diagnostics.push(e)
    }
  }
  // 低 4 位：互斥，按 JD & 0x0f 的取值直接匹配对应条目
  const low = jd & 0x0f
  if (low !== 0) {
    const hit = CJT188_JD_ENTRIES.find((e) => e.mask === 0x0f && parseInt(e.code, 16) === low)
    if (hit) diagnostics.push(hit)
  }
  return { bits, diagnostics }
}

/** 协议中文名 */
export const PROTOCOL_ERROR_PROTOCOL_LABEL: Record<ProtocolErrorProtocol, string> = {
  dlt645: 'DL/T 645-2007',
  cjt188: 'CJ/T 188-2018',
}

/**
 * 把某个协议的完整码表转成统一表格行（供「异常码速查」页渲染）。
 *
 * DLT645 的码表是位标志，与 Modbus 的「一码一条」形态不同——列表里按
 * bit 逐行列出，并标注该位可与哪些位并存，避免用户以为 ERR 只有一个错误。
 */
export function buildProtocolErrorEntries(protocol: ProtocolErrorProtocol): ProtocolErrorEntry[] {
  if (protocol === 'dlt645') {
    return DLT645_ERR_BITS.map((b) => ({
      code: `ERR bit${b.bit}`,
      name: b.name,
      meaning: b.meaning,
      cause: b.cause,
      solution: b.solution,
    }))
  }
  const rows: ProtocolErrorEntry[] = CJT188_ST_BITS.map((b) => ({
    code: `ST bit${b.bit}`,
    name: b.name,
    meaning: b.meaning,
    cause: b.cause,
    solution: b.solution,
  }))
  rows.push(
    ...CJT188_JD_ENTRIES.map((d) => ({
      code: `JD ${d.code}`,
      name: d.name,
      meaning: d.meaning,
      cause: d.cause,
      solution: d.solution,
    })),
  )
  return rows
}

/**
 * 各协议码表的补充说明（页面上作为提示展示）。
 * DLT645 特别说明 ERR 是位标志——这是最容易误解的一点。
 */
export const PROTOCOL_ERROR_NOTES: Record<ProtocolErrorProtocol, string> = {
  dlt645:
    'DL/T 645-2007 的错误信息字 ERR 是**位标志**而非码值表：一个字节可同时表示多个错误（各位独立置位）。'
    + '从站异常应答帧格式为 68 A0..A5 68 Cx 01 ERR CS 16，其中控制码 D6 位=1 表示异常应答、数据域长度 L=01H，'
    + '数据域唯一字节即为 ERR（未加 33H 加密）。bit1~bit6 之外的情形统一归入 bit0「其他错误」。',
  cjt188:
    'CJ/T 188-2018 的状态 ST 占 2 字节：第一字节是位标志（阀门开关/阀门状态/电池电压），'
    + '第二字节是诊断信息码 JD（低 4 位为电池/换能器/E2PROM 类互斥码，高 4 位为温度传感器类可并存码）。'
    + '从站异常应答帧的数据域为 SER(1B) + 状态ST(2B)，L=03H。'
    + '注意：标准只规定第二字节为厂商定义，JD 具体取值随厂商而异，下表为主流超声水表实现的常见取值。',
}
