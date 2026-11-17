/**
 * 协议工具失败分类（机器码）→ 现场可读中文的唯一映射
 *
 * 与 `meterErrorCodes.ts` 的分工（两模块名字都带"错误"会混，故按职责区分命名）：
 * - 本模块：**运行时失败分类**。后端只回机器码（timeout / crc / ...），中文由前端出。
 * - `meterErrorCodes.ts`：**表计协议异常码知识库**（DL/T 645 的 ERR 位、CJ/T 188 的 ST 字节），
 *   是「协议规范里的码表」，与请求成败无关。
 *
 * 机器码契约见 `api/protocolTools/types.ts` 的 `SendResult.error`
 * （timeout / crc / exception / bus_offline / bus_busy / invalid_frame / bus_not_found）。
 * 此前这张表在**两处**各写了一份，且同一机器码给出了不同说法：
 *
 *   | 机器码        | packet/parts/stepDebugTypes.ts | packet/PacketEditor.vue |
 *   |---------------|--------------------------------|--------------------------|
 *   | exception     | 设备返回异常码                  | 设备返回异常帧            |
 *   | bus_offline   | 总线离线                        | 总线不可用                |
 *   | invalid_frame | 帧格式非法                      | 请求帧非法                |
 *
 * 同一个 `bus_offline` 在单步调试里显示「总线离线」、在报文编辑里显示「总线不可用」，
 * 用户会以为是两种不同故障。收敛到本模块后只有一套说法。
 *
 * 选词依据（不是审美偏好）：
 * - `exception` 取「异常码」：Modbus 异常应答携带的是 1 字节异常码（01 非法功能…），
 *   不存在「异常帧」这种说法，且字段本身叫异常码；
 * - `bus_offline` 取「总线不可用」：后端语义是**端口打不开/连接失败**
 *   （`BusErrorKind::BusOffline`），不是「从站在线与否」；
 * - `bus_busy` 取「总线被采集占用」：后端在采集引擎占用总线时直接返回该码而不抢占，
 *   点明「被采集占用」才能让用户想到去暂停采集，而不是去查接线；
 * - `invalid_frame` 取「请求帧非法」：该码只在**发送前**做结构校验时返回（400），
 *   此时根本没有响应帧，说「帧格式非法」容易被误读成收到的帧有问题。
 *
 * 未知机器码原样透出（后端新增分类时前端不会退化成空白，运维仍能看到原始标识）。
 */

/** 协议工具失败分类 → 中文标签 */
export const PROTOCOL_FAILURE_LABELS: Readonly<Record<string, string>> = {
  timeout: '超时无响应',
  crc: 'CRC 校验失败',
  exception: '设备返回异常码',
  bus_offline: '总线不可用',
  bus_busy: '总线被采集占用',
  invalid_frame: '请求帧非法',
  bus_not_found: '总线不存在',
}

/**
 * 机器码转中文标签；空值返回空串，未知机器码原样返回。
 *
 * 空值返回空串而不是 undefined，是为了让调用方可以直接拼进模板
 * （`${label}：${detail}`），不必各自再判空一次。
 */
export function protocolFailureLabel(err?: string | null): string {
  if (!err) return ''
  return PROTOCOL_FAILURE_LABELS[err] ?? err
}
