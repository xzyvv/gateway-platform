/**
 * 独立 Modbus TCP 从站模拟器（Node 原生 net，无第三方依赖）。
 *
 * 为什么需要它
 * ------------
 * 物理硬件部署前，必须先在笔记本上验证「配置 → 总线 → 协议帧 → 解码 → 入队 → 查询」
 * 这条**生产链路**真的能跑通。但仓库里已有的自动化各缺一块：
 *   - Playwright E2E：跑的是真 gatewayd，但配置为空（buses/points 都是 []），总线从未打开；
 *   - gateway-api/tests/e2e_journey.rs：用 FakeBus 替换驱动交换层，不走生产传输层与调度循环；
 *   - south/protocol 各协议的 tests.rs：只测帧编解码，不测「配置 → 采集 → 入队」。
 *
 * 本模拟器提供一个**真实 TCP 从站**，让真实 gatewayd 用真实 Modbus TCP 帧去采，
 * 从而在不接物理设备的前提下补上上述全部缺口。
 *
 * 帧格式（Modbus TCP / MBAP，与 gateway-core/src/south/protocol/modbus/transaction/frame.rs
 * 的 build_mbap_frame 保持一致）：
 *   请求：tx_id(2) | protocol_id(2)=0 | length(2) | unit_id(1) | fc(1) | pdu
 *   响应：tx_id(2) | protocol_id(2)=0 | length(2) | unit_id(1) | fc(1) | data
 *   length = 1(unit_id) + 1(fc) + data.len()；整帧长度 = 6 + length
 *   例：FC03 读从站 1 的 0x0010 起 2 个寄存器 → length=6，整帧 12 字节
 */

import net from 'node:net';

/** Modbus 异常码 */
export const EX_ILLEGAL_FUNCTION = 0x01;
export const EX_ILLEGAL_ADDRESS = 0x02;
export const EX_ILLEGAL_VALUE = 0x03;

/**
 * 启动一个 Modbus TCP 从站。
 *
 * @param {object} opts
 * @param {string} [opts.host]      监听地址，默认 127.0.0.1
 * @param {number} [opts.port]      监听端口，默认 0（由系统分配，避免与真机/其他测试抢端口）
 * @param {number[]} [opts.holding] 保持寄存器初值（u16 数组）
 * @param {number[]} [opts.input]   输入寄存器初值（u16 数组）
 * @param {number[]} [opts.coils]   线圈初值（0/1 数组）
 * @returns {Promise<{port:number, address:string, stats:object, close:()=>Promise<void>}>}
 */
export function startSlave(opts = {}) {
  const host = opts.host ?? '127.0.0.1';
  const holding = Uint16Array.from(opts.holding ?? []);
  const input = Uint16Array.from(opts.input ?? []);
  const coils = Uint8Array.from(opts.coils ?? []);

  const stats = { requests: 0, byFunction: {}, exceptions: 0 };

  const server = net.createServer((socket) => {
    socket._buf = Buffer.alloc(0);
    socket.on('error', () => { /* 客户端断开：忽略，避免冒烟脚本被非致命错误打断 */ });
    socket.on('data', (chunk) => {
      socket._buf = Buffer.concat([socket._buf, chunk]);
      // 按 MBAP 的 length 字段切帧：整帧 = 6 + length
      for (;;) {
        if (socket._buf.length < 6) break;
        const length = socket._buf.readUInt16BE(4);
        const total = 6 + length;
        if (socket._buf.length < total) break;
        const frame = socket._buf.subarray(0, total);
        socket._buf = socket._buf.subarray(total);
        const response = buildResponse(frame, { holding, input, coils }, stats);
        if (response) socket.write(response);
      }
    });
  });

  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(opts.port ?? 0, host, () => {
      const { port } = server.address();
      resolve({
        port,
        address: `${host}:${port}`,
        stats,
        close: () =>
          new Promise((done) => {
            server.close(() => done());
          }),
      });
    });
  });
}

/** 由请求帧构造响应帧；广播（unit_id=0）不响应 */
function buildResponse(frame, regs, stats) {
  if (frame.length < 8) return null;
  const txId = frame.readUInt16BE(0);
  const unitId = frame[6];
  const fc = frame[7];
  const pdu = frame.subarray(8);

  stats.requests += 1;
  stats.byFunction[fc] = (stats.byFunction[fc] ?? 0) + 1;

  // 广播地址：不回包
  if (unitId === 0) return null;

  let respFc = fc;
  let data;
  try {
    data = handlePdu(fc, pdu, regs);
  } catch (err) {
    if (!(err instanceof ModbusException)) throw err;
    stats.exceptions += 1;
    respFc = fc | 0x80;
    data = Buffer.from([err.code]);
  }
  return mbap(txId, unitId, respFc, data);
}

/**
 * MBAP 封装：data **不含 fc**，fc 单独作为一字节插入 unit_id 之后。
 *
 * 长度语义必须与 gateway-core 的 build_mbap_frame / read_mbap_response 严格对齐：
 *   length = 1(unit_id) + 1(fc) + data.len()
 *   整帧 = 6 + length
 * 若此处漏掉 fc 字节，网关侧会把响应数据的首字节当成 fc，导致
 * parse_read_response 的 fc 匹配与 byte_count 校验全部错位。
 */
function mbap(txId, unitId, fc, data) {
  const header = Buffer.alloc(6);
  header.writeUInt16BE(txId, 0);
  header.writeUInt16BE(0, 2); // protocol_id：Modbus
  header.writeUInt16BE(data.length + 2, 4); // length = unit_id(1) + fc(1) + data
  return Buffer.concat([header, Buffer.from([unitId, fc]), data]);
}

class ModbusException extends Error {
  constructor(code) {
    super(`modbus exception ${code}`);
    this.code = code;
  }
}

/** 处理 PDU，返回**不含 fc** 的响应数据 */
function handlePdu(fc, pdu, regs) {
  switch (fc) {
    case 0x01: // 读线圈
      return readBits(fc, pdu, regs.coils);
    case 0x02: // 读离散输入
      return readBits(fc, pdu, regs.coils);
    case 0x03: // 读保持寄存器
      return readRegisters(fc, pdu, regs.holding);
    case 0x04: // 读输入寄存器
      return readRegisters(fc, pdu, regs.input);
    case 0x06: // 写单个寄存器
      return writeSingleRegister(fc, pdu, regs.holding);
    case 0x10: // 写多个寄存器
      return writeMultipleRegisters(fc, pdu, regs.holding);
    default:
      throw new ModbusException(EX_ILLEGAL_FUNCTION);
  }
}

function readRegisters(fc, pdu, table) {
  if (pdu.length < 4) throw new ModbusException(EX_ILLEGAL_VALUE);
  const addr = pdu.readUInt16BE(0);
  const count = pdu.readUInt16BE(2);
  if (count === 0 || count > 125) throw new ModbusException(EX_ILLEGAL_VALUE);
  if (addr + count > table.length) throw new ModbusException(EX_ILLEGAL_ADDRESS);
  const data = Buffer.alloc(2 * count);
  for (let i = 0; i < count; i += 1) data.writeUInt16BE(table[addr + i], i * 2);
  return Buffer.concat([Buffer.from([2 * count]), data]);
}

function readBits(fc, pdu, table) {
  if (pdu.length < 4) throw new ModbusException(EX_ILLEGAL_VALUE);
  const addr = pdu.readUInt16BE(0);
  const count = pdu.readUInt16BE(2);
  if (count === 0 || count > 2000) throw new ModbusException(EX_ILLEGAL_VALUE);
  if (addr + count > table.length) throw new ModbusException(EX_ILLEGAL_ADDRESS);
  const byteCount = Math.ceil(count / 8);
  const data = Buffer.alloc(byteCount);
  for (let i = 0; i < count; i += 1) {
    if (table[addr + i]) data[i >> 3] |= 1 << (i & 7);
  }
  return Buffer.concat([Buffer.from([byteCount]), data]);
}

function writeSingleRegister(fc, pdu, table) {
  if (pdu.length < 4) throw new ModbusException(EX_ILLEGAL_VALUE);
  const addr = pdu.readUInt16BE(0);
  const value = pdu.readUInt16BE(2);
  if (addr >= table.length) throw new ModbusException(EX_ILLEGAL_ADDRESS);
  table[addr] = value;
  return Buffer.from(pdu.subarray(0, 4)); // 回显 addr + value
}

function writeMultipleRegisters(fc, pdu, table) {
  if (pdu.length < 5) throw new ModbusException(EX_ILLEGAL_VALUE);
  const addr = pdu.readUInt16BE(0);
  const count = pdu.readUInt16BE(2);
  const byteCount = pdu[4];
  if (count === 0 || count > 123) throw new ModbusException(EX_ILLEGAL_VALUE);
  if (byteCount !== count * 2) throw new ModbusException(EX_ILLEGAL_VALUE);
  if (addr + count > table.length) throw new ModbusException(EX_ILLEGAL_ADDRESS);
  for (let i = 0; i < count; i += 1) table[addr + i] = pdu.readUInt16BE(5 + i * 2);
  return Buffer.from(pdu.subarray(0, 4)); // 回显 addr + count
}
