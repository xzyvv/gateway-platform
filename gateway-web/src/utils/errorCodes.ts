/**
 * 错误码字典（前端侧）
 *
 * 后端 `ApiResponse{ code, message, data }` 里的 `code` 是稳定契约，`message`
 * 是面向客户的中文文案。二者分工：
 *   - message 有值时**永远优先**（后端更清楚是哪一段配置/哪个点位出的问题）
 *   - message 缺失或为空时，用本字典兜底，避免把 `code: 8000` 这种裸数字丢给用户
 *
 * 码表来源：后端 `gateway-api/src/web/error_codes.rs` 的 `ALL_CODES`（唯一来源）。
 * 后端新增码时会先登记到那张表，本文件必须同步补文案；反之本文件不应出现
 * 后端表里没有的码。
 *
 * 注意：这里只做「码 → 文案」映射，不做 HTTP 状态码推断；HTTP 4xx/5xx 的
 * 语义在 http.ts 拦截器里按状态码分支处理（401/4021/429 有专门分支）。
 */

/** 稳定错误码常量，供拦截器分支引用，避免魔法数字散落 */
export const ERR = {
  /** 成功 */
  OK: 0,
  /** 配置错误 或 请求体 JSON 解析失败（由客户端提交触发） */
  CONFIG_OR_JSON: 1000,
  /** 本地 IO 失败（磁盘/文件/串口句柄等） */
  IO: 2000,
  /** 协议层错误（帧非法 / CRC / 编解码） */
  PROTOCOL: 3000,
  /** 设备或总线在超时内没有应答 */
  NO_REPLY: 3001,
  /** 参数非法（ApiError::bad_request） */
  BAD_REQUEST: 4000,
  /** 未指定总线（协议工具会话） */
  BUS_MISSING: 4001,
  /** 参数校验未通过：点位字段 / 协议不支持 共用 */
  INVALID_PARAM: 4002,
  /** 配置应用失败（校验通过但落盘/热重载失败） */
  CONFIG_APPLY_FAILED: 4003,
  /** 资源不存在（设备/总线/版本等） */
  NOT_FOUND: 4004,
  /** 用户域：用户名/显示名不合法或重复 */
  USER_INVALID: 4005,
  /** 用户域：用户不存在 */
  USER_NOT_FOUND: 4006,
  /** 用户域：角色不合法 */
  ROLE_INVALID: 4007,
  /** 公式求值失败（用户配置的公式非法，可纠正） */
  FORMULA_INVALID: 4008,
  /** id 冲突（回收站恢复点位时目标 id 已存在） */
  DUPLICATE_ID: 4009,
  /** 资源状态冲突（HTTP 409）：如模拟消费者已在运行、MQTT 已启用 */
  STATE_CONFLICT: 4010,
  /** 授权配额不足（v2.0 档位限制，授权有效但档位不够） */
  QUOTA_EXCEEDED: 4021,
  /** 总线忙（协议工具会话租约冲突；HTTP 200 + 业务码） */
  BUS_BUSY: 4090,
  /** 请求 Content-Type 不受支持 */
  UNSUPPORTED_MEDIA_TYPE: 4150,
  /** 请求体结构不合法 */
  UNPROCESSABLE: 4220,
  /** 请求过于频繁（限流） */
  RATE_LIMITED: 4290,
  /** 总线离线（协议工具会话；HTTP 200 + 业务码） */
  BUS_OFFLINE: 4503,
  /** MQTT 北向发布失败 */
  MQTT: 5000,
  /** 上传的证书无法解析 */
  CERT_PARSE_FAILED: 5001,
  /** 本地队列 / 存储故障（SQLite 读写失败、库被锁等） */
  QUEUE_ERROR: 5002,
  /** 依赖能力暂不可用 */
  SERVICE_UNAVAILABLE: 5003,
  /** 点位写入失败（明确失败：能证明写没有生效，可安全重试） */
  WRITE_FAILED: 5101,
  /** 点位写入结果未知（设备可能已执行，禁止自动重试） */
  WRITE_UNCERTAIN: 5102,
  /** 设备分组：名称非法/重复 */
  GROUP_INVALID: 6001,
  /** 设备分组不存在 */
  GROUP_NOT_FOUND: 6002,
  /** License 未导入（网关按兜底配额运行，超限需导入授权） */
  LICENSE_REQUIRED: 7001,
  /** License 失效（过期/设备码不匹配/篡改/验签失败） */
  LICENSE_INVALID: 7003,
  /** 鉴权失效 / token 过期 */
  UNAUTHORIZED: 8000,
  /** API Key 缺少写权限（只读密钥调用写接口） */
  API_KEY_NO_WRITE: 8003,
  /** API Key 权限范围不足（如缺 diagnostics:exec） */
  API_KEY_SCOPE: 8004,
  /** 拒绝通过普通配置写修改 web 凭据等敏感字段 */
  SENSITIVE_FIELD_DENIED: 8005,
  /** 实时 WebSocket 连接数超限 */
  REALTIME_LIMIT: 8101,
  /** 网关内部错误 */
  INTERNAL: 9999,
} as const

export interface ErrorCodeEntry {
  /** 兜底文案（短，可直接进 toast） */
  message: string
  /** 可选处置建议，拼在 message 后用「，」连接 */
  hint?: string
}

export const ERROR_CODES: Readonly<Record<number, ErrorCodeEntry>> = {
  [ERR.CONFIG_OR_JSON]: {
    message: '配置或请求内容解析失败',
    hint: '请检查提交的配置/JSON 结构是否完整；刚升级过版本可先刷新页面再试',
  },
  [ERR.IO]: {
    message: '本地读写失败',
    hint: '请检查磁盘空间与文件权限；反复出现请查看系统日志',
  },
  [ERR.PROTOCOL]: {
    message: '协议通讯异常',
    hint: '设备有应答但报文不符合协议：请核对从站地址、功能码与寄存器/点号配置',
  },
  [ERR.NO_REPLY]: {
    message: '设备未应答',
    hint: '超时内没有收到回应：请确认设备在线、从站地址与接线/端口正确',
  },
  [ERR.BAD_REQUEST]: {
    message: '请求参数不合法',
    hint: '请检查表单填写是否完整、格式是否正确',
  },
  [ERR.BUS_MISSING]: {
    message: '未指定总线',
    hint: '请先选择总线再重试',
  },
  [ERR.INVALID_PARAM]: {
    message: '参数校验未通过',
    hint: '请检查协议、地址、从站地址、采集周期等字段',
  },
  [ERR.CONFIG_APPLY_FAILED]: {
    message: '配置保存失败',
    hint: '配置校验通过但写入/热重载失败，请查看系统日志后重试',
  },
  [ERR.NOT_FOUND]: {
    message: '目标资源不存在',
    hint: '可能已被删除，请刷新页面后重试',
  },
  [ERR.USER_INVALID]: {
    message: '用户名或显示名不合法',
    hint: '不能为空且不可与已有用户重复',
  },
  [ERR.USER_NOT_FOUND]: {
    message: '用户不存在',
    hint: '可能已被删除，请刷新后重试',
  },
  [ERR.ROLE_INVALID]: {
    message: '角色不合法',
    hint: '请选择系统中已存在的角色',
  },
  [ERR.DUPLICATE_ID]: {
    message: 'id 已存在',
    hint: '请先删除同名对象，或改用其它 id',
  },
  [ERR.STATE_CONFLICT]: {
    message: '当前状态不允许该操作',
    hint: '请先停止/关闭冲突的功能（如正在运行的模拟消费者或已启用的 MQTT）后重试',
  },
  [ERR.UNAUTHORIZED]: {
    message: '登录状态已失效',
    hint: '请重新登录',
  },
  [ERR.QUOTA_EXCEEDED]: {
    message: '当前授权配额不足',
    hint: '请联系供应商升级授权',
  },
  [ERR.BUS_BUSY]: {
    message: '总线正被占用',
    hint: '协议调试会话未释放，请先停止调试或稍后重试',
  },
  [ERR.UNSUPPORTED_MEDIA_TYPE]: {
    message: '请求格式不受支持',
    hint: '请使用 JSON 提交（Content-Type: application/json）',
  },
  [ERR.UNPROCESSABLE]: {
    message: '请求内容不合法',
    hint: '请检查提交字段的类型与结构是否与接口要求一致',
  },
  [ERR.RATE_LIMITED]: {
    message: '请求过于频繁',
    hint: '请稍等几秒再操作，避免连续点击',
  },
  [ERR.BUS_OFFLINE]: {
    message: '总线已离线',
    hint: '请检查网关到设备的接线与串口/TCP 连通性',
  },
  [ERR.MQTT]: {
    message: 'MQTT 发布失败',
    hint: '请检查 MQTT 服务地址、凭据与网络连通性；数据会在本地队列重试',
  },
  [ERR.CERT_PARSE_FAILED]: {
    message: '证书解析失败',
    hint: '请确认上传的是有效的 PEM 证书与私钥',
  },
  [ERR.QUEUE_ERROR]: {
    message: '本地队列或存储故障',
    hint: '请查看系统日志确认磁盘空间与数据库状态后重试',
  },
  [ERR.SERVICE_UNAVAILABLE]: {
    message: '依赖能力暂不可用',
    hint: '请稍后重试；反复出现请查看系统日志',
  },
  [ERR.WRITE_FAILED]: {
    message: '点位写入失败',
    hint: '请确认设备在线、点位允许写入、地址与数据类型正确',
  },
  [ERR.WRITE_UNCERTAIN]: {
    message: '点位写入结果未知',
    hint: '设备可能已执行，请先读回核实，不要自动重试',
  },
  [ERR.GROUP_INVALID]: {
    message: '设备分组名称不合法',
    hint: '组名不能为空且不可与已有分组重复',
  },
  [ERR.GROUP_NOT_FOUND]: {
    message: '设备分组不存在',
    hint: '可能已被删除，请刷新后重试',
  },
  [ERR.FORMULA_INVALID]: {
    message: '公式求值失败',
    hint: '请检查点位公式的语法、变量名与量程，修正后再保存',
  },
  [ERR.LICENSE_REQUIRED]: {
    message: '尚未导入授权文件',
    hint: '请在「授权管理」中导入 .lic 文件',
  },
  [ERR.LICENSE_INVALID]: {
    message: '授权已失效',
    hint: '请检查有效期与设备码，必要时重新导入授权文件',
  },
  [ERR.API_KEY_NO_WRITE]: {
    message: 'API 密钥没有写权限',
    hint: '请改用带写权限的密钥',
  },
  [ERR.API_KEY_SCOPE]: {
    message: 'API 密钥权限范围不足',
    hint: '该接口需要更高 scope，请换用具备对应权限的密钥',
  },
  [ERR.SENSITIVE_FIELD_DENIED]: {
    message: '敏感字段不允许这样修改',
    hint: 'Web 登录凭据等敏感项请到「系统设置 → 安全」中单独修改',
  },
  [ERR.REALTIME_LIMIT]: {
    message: '实时连接数已达上限',
    hint: '请关闭部分实时页面或稍后再试',
  },
  [ERR.INTERNAL]: {
    message: '网关内部错误',
    hint: '请稍后重试；反复出现请到「系统日志」查看详情',
  },
}

/**
 * 后端少数错误码直接返回英文 message（group not found / license required 等），
 * 原样透出等于把开发者文案丢给现场操作员。按 message 精确匹配译成中文；
 * 未收录的后端中文文案照旧优先原样透出（见 describeErrorCode 的优先级说明）。
 */
const BACKEND_MESSAGE_ZH: Readonly<Record<string, string>> = {
  'group not found': '设备分组不存在，可能已被删除',
  'user not found': '用户不存在，可能已被删除',
  'api key lacks write scope': '当前 API 密钥没有写权限，请改用带写权限的密钥',
  'license required': '尚未导入授权文件，请在「授权管理」中导入 .lic 文件',
  'license expired or invalid': '授权已失效，请检查有效期或重新导入授权文件',
  // E 批：NoReply 现在走 504（上游未响应），后端 Display 原文是这句
  'no reply from device': '设备未响应（超时内没有应答），请检查设备在线、从站地址与接线',
}

/** 码是否存在兜底文案（用于区分"已知业务错"与"未知码"） */
export function isKnownErrorCode(code: number): boolean {
  return Object.prototype.hasOwnProperty.call(ERROR_CODES, code)
}

/**
 * 生成用户可读的错误文案。
 *
 * 优先级：后端 message（去空白后非空）> 字典 message(+hint) > `错误码 N`。
 * 传 0 或非有限数字时按未知码处理，调用方不应把 0 当错误。
 */
export function describeErrorCode(code: number, backendMessage?: string | null): string {
  const raw = typeof backendMessage === 'string' ? backendMessage.trim() : ''
  if (raw) return BACKEND_MESSAGE_ZH[raw.toLowerCase()] ?? raw

  const entry = Number.isFinite(code) ? ERROR_CODES[code] : undefined
  if (entry) return entry.hint ? `${entry.message}，${entry.hint}` : entry.message

  return `错误码 ${code}`
}

/** 通知中心用的「错误码详情」：是什么（label）+ 下一步做什么（suggestion） */
export interface ErrorCodeDetail {
  label: string
  suggestion: string
}

/**
 * HTTP 状态码的人话映射。
 *
 * http.ts 拦截器把 HTTP 状态码作为通知的 code 透传（此前只显示裸数字 500/502），
 * 这里给出非技术用户能懂的「是什么 + 下一步」。只列控制台会真实遇到的：
 * 网关重启期的 502/503、过载 504、参数错 400、权限 403 等。
 */
const HTTP_STATUS_DETAILS: Readonly<Record<number, ErrorCodeDetail>> = {
  400: { label: '请求参数有误', suggestion: '请检查表单填写是否完整、格式是否正确' },
  403: { label: '没有操作权限', suggestion: '当前账号不是管理员，请联系管理员处理' },
  404: { label: '资源不存在', suggestion: '可能已被删除，请刷新页面后重试' },
  409: { label: '资源冲突', suggestion: '数据已被修改或重复提交，请刷新后重试' },
  413: { label: '提交内容过大', suggestion: '请减少单次提交的数据量（如分批导入）' },
  429: { label: '请求过于频繁', suggestion: '请稍等几秒再操作，避免连续点击' },
  500: { label: '网关内部错误', suggestion: '请稍后重试；反复出现请到「系统日志」查看错误详情' },
  502: { label: '网关服务未就绪', suggestion: '网关可能正在重启，请等待几秒后刷新页面' },
  503: { label: '服务暂时不可用', suggestion: '网关可能过载或正在重启，请稍后刷新页面' },
  // 504 现在主要来自「设备/总线未应答」（GatewayError::NoReply → 504），
  // 也可能是网关真的过载或代理超时，所以文案同时覆盖两端。
  504: {
    label: '上游未响应',
    suggestion:
      '设备或总线未在超时内应答：请确认设备在线、从站地址与接线正确；若所有页面都超时再查网关负载',
  },
}

/** 是否属于 HTTP 状态码区间（100–599）。业务码（4021/8000 等）都在区间外，不会误判 */
export function isHttpStatusCode(code: number): boolean {
  return Number.isInteger(code) && code >= 100 && code <= 599
}

/**
 * 错误码 → 详情（人话解释 + 排查建议）。通知中心渲染用。
 *
 * 命中 HTTP 状态码或已知业务码时返回详情；未知业务码返回 undefined，
 * 调用方回退为「错误码 N」展示（与 describeErrorCode 的兜底语义一致）。
 */
export function describeErrorDetail(code: number): ErrorCodeDetail | undefined {
  if (!Number.isFinite(code)) return undefined
  if (isHttpStatusCode(code)) return HTTP_STATUS_DETAILS[code]
  const entry = ERROR_CODES[code]
  if (entry) return { label: entry.message, suggestion: entry.hint ?? '' }
  return undefined
}
