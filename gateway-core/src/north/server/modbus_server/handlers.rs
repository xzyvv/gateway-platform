//! Modbus 请求处理：FC 分发 / 位读 / 寄存器读写 / 异常响应。
//!
//! 从 `modbus_server.rs` 拆出，只为控制单文件行数（C24 行数预算），行为不变。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use super::ModbusServer;
use super::{
    EX_ILLEGAL_DATA_ADDRESS, EX_ILLEGAL_DATA_VALUE, EX_ILLEGAL_FUNCTION, FC_READ_COILS,
    FC_READ_DISCRETE, FC_READ_HOLDING, FC_READ_INPUT, FC_WRITE_MULTIPLE, FC_WRITE_SINGLE,
};

impl ModbusServer {
    /// 根据 function_code 分发请求（写操作在分发层做白名单校验）
    pub(crate) fn handle_request(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
        write_enabled: bool,
        writable_addrs: &HashSet<u16>,
    ) -> Vec<u8> {
        match fc {
            FC_READ_COILS | FC_READ_DISCRETE => Self::handle_read_bits(fc, data, registers),
            FC_READ_HOLDING | FC_READ_INPUT => Self::handle_read(fc, data, registers),
            FC_WRITE_SINGLE => {
                // 过短报文放行给下游报格式错误；默认 write_enabled=false 一律拒绝
                let allowed = write_enabled
                    && (data.len() < 2
                        || writable_addrs.contains(&u16::from_be_bytes([data[0], data[1]])));
                if !allowed {
                    return Self::exception(fc, EX_ILLEGAL_DATA_ADDRESS);
                }
                Self::handle_write_single(fc, data, registers)
            }
            FC_WRITE_MULTIPLE => {
                // 整批原子校验：任一地址不可写即整批拒绝，绝不部分写入
                let allowed = write_enabled
                    && (data.len() < 4 || {
                        let start = u16::from_be_bytes([data[0], data[1]]);
                        let qty = u16::from_be_bytes([data[2], data[3]]);
                        qty == 0
                            || qty > 123
                            // DD-43：与读路径统一用 saturating_add——跨越 0xFFFF 的
                            // 地址饱和到 0xFFFF（读侧同样如此），不再回绕到 0。
                            || (0..qty).all(|i| writable_addrs.contains(&start.saturating_add(i)))
                    });
                if !allowed {
                    return Self::exception(fc, EX_ILLEGAL_DATA_ADDRESS);
                }
                Self::handle_write_multiple(fc, data, registers)
            }
            _ => Self::exception(fc, EX_ILLEGAL_FUNCTION),
        }
    }

    /// FC01/FC02: 读线圈/离散输入
    /// 请求 data: starting_addr(2) + quantity(2)
    /// 响应 PDU: fc(1) + byte_count(1) + bits(ceil(quantity/8))
    ///
    /// 与寄存器共用地址空间：线圈地址 N 对应寄存器 N 的最低位。
    pub(crate) fn handle_read_bits(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        if data.len() < 4 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let starting_addr = u16::from_be_bytes([data[0], data[1]]);
        let quantity = u16::from_be_bytes([data[2], data[3]]);

        // Modbus 规范：FC01/FC02 quantity 范围 1-2000
        if quantity == 0 || quantity > 2000 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let byte_count = (quantity as usize).div_ceil(8);
        let mut values = vec![0u8; byte_count];
        let reg_map = registers.read();
        for i in 0..quantity as usize {
            let addr = starting_addr.saturating_add(i as u16);
            // 寄存器不存在视为断开（0），与 FC03 缺省 0 的语义一致
            let on = reg_map.get(&addr).is_some_and(|v| v & 1 != 0);
            if on {
                values[i / 8] |= 1 << (i % 8);
            }
        }
        drop(reg_map);

        let mut resp = Vec::with_capacity(2 + values.len());
        resp.push(fc);
        resp.push(byte_count as u8);
        resp.extend_from_slice(&values);
        resp
    }

    /// FC03/FC04: 读保持/输入寄存器
    /// 请求 data: starting_addr(2) + quantity(2)
    /// 响应 PDU: fc(1) + byte_count(1) + values(quantity*2)
    pub(crate) fn handle_read(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        if data.len() < 4 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let starting_addr = u16::from_be_bytes([data[0], data[1]]);
        let quantity = u16::from_be_bytes([data[2], data[3]]);

        // Modbus 规范：FC03/FC04 quantity 范围 1-125
        if quantity == 0 || quantity > 125 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let reg_map = registers.read();
        let mut values = Vec::with_capacity(quantity as usize * 2);
        for i in 0..quantity {
            let addr = starting_addr.saturating_add(i);
            let val = reg_map.get(&addr).copied().unwrap_or(0);
            values.extend_from_slice(&val.to_be_bytes());
        }
        drop(reg_map);

        let mut resp = Vec::with_capacity(2 + values.len());
        resp.push(fc);
        resp.push(values.len() as u8);
        resp.extend_from_slice(&values);
        resp
    }

    /// FC06: 写单个寄存器
    /// 请求 data: addr(2) + value(2)
    /// 响应 PDU: fc(1) + addr(2) + value(2) （回显请求）
    pub(crate) fn handle_write_single(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        if data.len() < 4 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let addr = u16::from_be_bytes([data[0], data[1]]);
        let value = u16::from_be_bytes([data[2], data[3]]);

        registers.write().insert(addr, value);

        let mut resp = Vec::with_capacity(5);
        resp.push(fc);
        resp.extend_from_slice(&addr.to_be_bytes());
        resp.extend_from_slice(&value.to_be_bytes());
        resp
    }

    /// FC16: 写多个寄存器
    /// 请求 data: addr(2) + quantity(2) + byte_count(1) + values(quantity*2)
    /// 响应 PDU: fc(1) + addr(2) + quantity(2) （回显请求头和数量）
    ///
    /// 整批写入在一次写锁内完成，避免主站读到半更新状态。
    pub(crate) fn handle_write_multiple(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        if data.len() < 5 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let starting_addr = u16::from_be_bytes([data[0], data[1]]);
        let quantity = u16::from_be_bytes([data[2], data[3]]);
        let byte_count = data[4] as usize;

        // Modbus 规范：FC16 quantity 范围 1-123（123*2+5 = 251 字节 PDU 上限）
        if quantity == 0 || quantity > 123 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }
        // byte_count 必须与 quantity 严格一致，否则视为非法报文
        if byte_count != quantity as usize * 2 || data.len() < 5 + byte_count {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let mut reg_map = registers.write();
        for i in 0..quantity as usize {
            let off = 5 + i * 2;
            let value = u16::from_be_bytes([data[off], data[off + 1]]);
            // DD-43：与 FC01~FC04 读路径统一——跨 0xFFFF 时饱和到 0xFFFF，
            // 不再回绕到 0；读写两侧对越界地址得到同一组寄存器。
            reg_map.insert(starting_addr.saturating_add(i as u16), value);
        }
        drop(reg_map);

        let mut resp = Vec::with_capacity(5);
        resp.push(fc);
        resp.extend_from_slice(&starting_addr.to_be_bytes());
        resp.extend_from_slice(&quantity.to_be_bytes());
        resp
    }

    /// 构建异常响应 PDU: (fc | 0x80) + exception_code
    pub(crate) fn exception(fc: u8, code: u8) -> Vec<u8> {
        vec![fc | 0x80, code]
    }
}
