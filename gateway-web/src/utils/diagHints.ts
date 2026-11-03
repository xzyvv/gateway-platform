/**
 * 错误文案可操作化映射表
 *
 * 后端 / 底层返回的错误多为英文技术描述（如 "Address already in use"），
 * 现场工程师看不懂也不知道下一步做什么。本模块把常见错误串映射为
 * 「可执行的中文处置建议」，拼在原始错误后展示，让用户知道该改哪里。
 *
 * 设计原则：
 *   - 只匹配「明确有处置动作」的错误，模糊错误不瞎给建议
 *   - 命中即返回建议，未命中返回空串（调用方自行决定是否展示原文）
 *   - 匹配用小写 includes，兼容大小写与前后多余空白
 */

interface HintRule {
  /** 子串（小写），命中即触发 */
  match: string
  /** 可执行的中文建议 */
  hint: string
}

const RULES: HintRule[] = [
  // —— 串口 / 端口占用 ——
  { match: 'address already in use', hint: '端口已被占用，请检查是否有另一个程序或网关实例正在使用该端口，关闭后重试' },
  { match: 'permission denied', hint: '没有访问该串口/端口的权限，请以管理员身份运行网关，或检查设备权限（如 /dev/ttyUSB* 的读写权限）' },
  { match: 'no such file or directory', hint: '串口/设备文件不存在，请确认 USB 转串口线已插好，或在「设备管理」中确认串口号' },
  { match: 'resource busy', hint: '串口正被其他程序占用，请关闭串口调试助手等可能占用串口的软件后重试' },

  // —— 连接超时 / 网络 ——
  { match: 'timed out', hint: '连接超时，请检查设备是否上电、通信线是否接反、波特率/校验位是否与设备一致' },
  // 中文错误串同样需要命中（部分后端文案/前端拼装文案已是中文）
  { match: '超时', hint: '连接超时，请检查设备是否上电、通信线是否接反、波特率/校验位是否与设备一致' },
  { match: 'connection refused', hint: '连接被拒绝，请确认目标设备 IP 和端口正确，且设备服务已启动' },
  { match: 'network is unreachable', hint: '网络不可达，请检查网关与设备是否在同一网段，或网线是否接好' },
  { match: 'econnreset', hint: '连接被对端重置，可能是设备主动断开或中间网络异常，请检查设备状态后重试' },

  // —— Modbus / 协议帧 ——
  { match: 'crc', hint: 'CRC 校验失败，通常是通信干扰或接线松动，请检查屏蔽线与接地，或降低波特率重试' },
  { match: 'checksum', hint: '校验和错误，可能是通信干扰或接线松动，请检查线缆与接地' },
  { match: 'illegal function', hint: '设备不支持该功能码，请确认点位配置的功能码（读/写）与设备手册一致' },
  { match: 'illegal data address', hint: '寄存器地址超出设备范围，请核对设备手册中的地址映射（注意 0-based / 1-based 偏移）' },
  { match: 'illegal data value', hint: '写入值超出设备允许范围，请检查数值类型与量程' },
  { match: 'slave device failure', hint: '从站设备返回异常，可能是设备故障或忙，请稍后重试或检查设备状态' },
  { match: '无响应', hint: '设备未应答，请确认从站 ID 正确、设备在线且通信参数（波特率/校验位）一致' },

  // —— 配置 / 数据 ——
  { match: 'not in whitelist', hint: '该点位未开启写入白名单，请先在点位列表中开启「允许写入」' },
  { match: 'already exists', hint: '名称/地址已存在，请更换后重试' },
  { match: 'template not found', hint: '设备引用的模板不存在，请在「设备模板」页确认模板名（注意是模板显示名，不是文件名）' },
  { match: 'device name', hint: '设备名必须唯一（设备名参与点位 ID 生成），请改成不同名称后重试' },
  // S8: 「collector for bus bus1 not found」此前被通用 not found 规则误判成「目标已被删除」。
  // 实际语义是「该总线还没有采集器」，应引导用户检查总线是否已启用/是否配置了设备。
  { match: 'collector for bus', hint: '该总线没有运行中的采集器，请确认总线已启用且已挂载设备' },
  { match: 'not found', hint: '目标不存在，可能已被删除，请刷新列表后重试' },
]

/**
 * 根据错误消息返回可执行的处置建议。
 * @returns 命中时返回建议文案，未命中返回空串
 */
export function diagHint(message: string): string {
  if (!message) return ''
  const lower = message.toLowerCase()
  for (const rule of RULES) {
    if (lower.includes(rule.match)) return rule.hint
  }
  return ''
}

/**
 * 把原始错误消息与可执行建议合并为一条完整文案。
 * 未命中建议时原样返回 message。
 *
 * 幂等：同一串会被多层（http 层归一化、页面层展示）各调一次，
 * 若已拼过同一条建议就直接返回，否则会出现「…（建议）（建议）」的重复括号。
 */
export function withDiagHint(message: string): string {
  if (!message) return message
  const hint = diagHint(message)
  if (!hint || message.includes(hint)) return message
  return `${message}（${hint}）`
}
