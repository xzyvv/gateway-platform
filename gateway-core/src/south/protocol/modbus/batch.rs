//! 批量合并算法与点位值解析

use crate::config::PointConfig;
use crate::engine::model::{PointDataType, Quality};

use crate::engine::model::DataPoint;
use crate::south::protocol::{convert_value, decode_binary_value as decode_value, reorder_bytes};

// ===== 批量合并算法 =====

/// 读取一个点位需要申请的寄存器数。
///
/// 必须与解码宽度同源：`parse_point_value` 只按 `data_type` 取字节数，
/// 若这里优先采用手填的 `register_count`，两者矛盾时（例如 `data_type: f32`
/// 却填 `register_count: 1`）会只申请 1 个寄存器，解码阶段判定越界，
/// 该点位静默返回 Bad、现场表现为"一直采不到数"。
///
/// `register_count` 仅在未指定 `data_type` 时兜底（历史配置可能只填它）。
pub(crate) fn read_register_count(p: &PointConfig) -> u16 {
    p.data_type_enum()
        .map(|d| d.register_count())
        .or(p.register_count)
        .unwrap_or(1)
}

pub(crate) struct Batch<'a> {
    pub(crate) start_addr: u16,
    pub(crate) count: u16,
    pub(crate) points: Vec<&'a PointConfig>,
}

/// 合并相邻点位地址（§9.1）
pub(crate) fn merge_batches<'a>(
    points: &[&'a PointConfig],
    gap_threshold: u16,
    max_count: u16,
) -> Vec<Batch<'a>> {
    let mut batches: Vec<Batch<'a>> = Vec::new();

    for p in points {
        let addr = p.address_num.unwrap_or(0);
        let reg_count = read_register_count(p);

        if let Some(last) = batches.last_mut() {
            let last_end = last.start_addr.saturating_add(last.count);
            let addr_end = addr.saturating_add(reg_count);
            let batch_end = last.start_addr.saturating_add(max_count);
            if addr >= last.start_addr
                && addr_end <= batch_end
                && addr.saturating_sub(last_end) <= gap_threshold
            {
                last.count = addr_end.saturating_sub(last.start_addr).max(last.count);
                last.points.push(p);
                continue;
            }
        }
        batches.push(Batch {
            start_addr: addr,
            count: reg_count,
            points: vec![p],
        });
    }

    batches
}

/// DD-11：只读采集计划视图（Web「调度优化」页展示用，不参与采集）。
///
/// 与真实采集路径共用 [`merge_batches`]：同样的 (slave_id, function_code) 分组、
/// 同样的寄存器宽度、同样的 gap 阈值与协议上限，避免前端再抄一份近似算法
/// （旧前端只按地址差 ≤5 合并，忽略从站/功能码/寄存器宽度，显示的优化率是假的）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeGroupInfo {
    pub slave_id: u8,
    pub function_code: u8,
    pub start_addr: u16,
    pub end_addr: u16,
    pub point_count: u16,
    pub point_ids: Vec<String>,
}

/// 计算一批点位的合并读取计划。
///
/// - 只对寄存器/线圈类读取有效；调用方负责先过滤出 Modbus 点位；
/// - `gap_threshold` 由运行时自适应统计给出（见 `Collector::merge_gap`）。
pub fn merge_plan(points: &[&PointConfig], gap_threshold: u16) -> Vec<MergeGroupInfo> {
    // 与采集循环一致：按 (slave_id, function_code, 点位级超时) 分组，
    // 不同超时的点位不会合进同一帧。
    let mut groups: std::collections::HashMap<(u8, u8, Option<u64>), Vec<&PointConfig>> =
        std::collections::HashMap::new();
    for p in points {
        let key = (
            p.slave_id.unwrap_or(1),
            p.function_code.unwrap_or(3),
            p.timeout_ms,
        );
        groups.entry(key).or_default().push(*p);
    }

    let mut out = Vec::new();
    for ((slave_id, function_code, _timeout), mut group) in groups {
        group.sort_by_key(|p| p.address_num.unwrap_or(0));
        // 与采集循环一致：线圈/离散输入上限 2000，寄存器上限 125
        let max_count = if function_code == 1 || function_code == 2 {
            2000
        } else {
            125
        };
        for batch in merge_batches(&group, gap_threshold, max_count) {
            let end_addr = batch
                .start_addr
                .saturating_add(batch.count)
                .saturating_sub(1);
            out.push(MergeGroupInfo {
                slave_id,
                function_code,
                start_addr: batch.start_addr,
                end_addr,
                point_count: batch.points.len() as u16,
                point_ids: batch.points.iter().map(|p| p.id.clone()).collect(),
            });
        }
    }
    out.sort_by_key(|g| (g.slave_id, g.function_code, g.start_addr));
    out
}

// ===== 数据解析 =====

pub(crate) fn parse_point_value(point: &PointConfig, regs: &[u16], batch_start: u16) -> DataPoint {
    let addr = point.address_num.unwrap_or(0);
    // 用 saturating_sub 而非裸减：正常由 merge_batches 保证 addr >= batch_start，
    // 但这是隐式契约，裸减一旦被破坏在 debug 构建下会直接 panic。
    let offset = addr.saturating_sub(batch_start) as usize;
    let data_type = point.data_type_enum().unwrap_or(PointDataType::U16);
    let byte_order = point.byte_order_enum();

    let count = data_type.register_count() as usize;
    if offset + count > regs.len() {
        return DataPoint::failed(
            &point.id,
            &point.device_id.clone().unwrap_or_else(|| point.bus.clone()),
            None,
            0,
            Quality::Bad,
            // 解码所需的寄存器数量超出批次范围，固定标为 DECODE
            "modbus_decode_failed:DECODE",
            0,
        );
    }

    let mut bytes = Vec::with_capacity(count * 2);
    for i in 0..count {
        let reg = regs[offset + i];
        bytes.push((reg >> 8) as u8);
        bytes.push((reg & 0xFF) as u8);
    }

    let ordered = reorder_bytes(&bytes, byte_order);

    let (value, raw_value, raw_type, quality) = match decode_value(&ordered, data_type) {
        Some((raw, t)) => {
            let value = convert_value(point, raw);
            (value, raw, t.to_string(), Quality::Good)
        }
        None => {
            tracing::warn!(
                "modbus decode failed: point {} data_type {:?} needs {} bytes but got {}",
                point.id,
                data_type,
                data_type.register_count() * 2,
                ordered.len()
            );
            (
                0.0,
                0.0,
                format!("{:?}_decode_err", data_type),
                Quality::Bad,
            )
        }
    };

    DataPoint {
        point_id: point.id.clone(),
        device_id: point.device_id.clone().unwrap_or_else(|| point.bus.clone()),
        value,
        raw_value,
        raw_type,
        quality,
        quality_reason: if quality != Quality::Good {
            "modbus_decode:DECODE".to_string()
        } else {
            String::new()
        },
        timestamp: DataPoint::now(),
        source_ts: DataPoint::now(),
        stale_count: 0,
        value_text: None,
    }
}

// ===== 多写合并（决策表 #1：多点写入口）=====

/// FC15 单帧最大线圈数（Modbus 规范上限 0x07B0）
pub(crate) const MAX_WRITE_COILS: u16 = 1968;
/// FC16 单帧最大寄存器数（Modbus 规范上限 0x007B）
pub(crate) const MAX_WRITE_REGISTERS: u16 = 123;

/// 写元件类型：决定同一片段用 FC15（线圈）还是 FC16（寄存器）下发。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WriteKind {
    /// 线圈（位元件）→ FC15 写多线圈
    Coil,
    /// 保持寄存器（字元件）→ FC16 写多寄存器
    Register,
}

/// 由点位配置的 `function_code` 判定可写元件类型。
///
/// 与 `ModbusCollector::write` 的功能码映射保持一致：
/// - `1 | 5 | 15` → 线圈（FC05/FC15 语义）
/// - `3 | 6 | 16` → 寄存器（FC06/FC16 语义）
/// - 其余（`0/2/4/...`）→ `None`，视为只读，由调用方逐点回退到 `write` 报错
pub(crate) fn write_kind_of(fc: u8) -> Option<WriteKind> {
    match fc {
        1 | 5 | 15 => Some(WriteKind::Coil),
        3 | 6 | 16 => Some(WriteKind::Register),
        _ => None,
    }
}

/// 一个写片段：目标区间 `[start_addr, start_addr + units)`，成员为 `items[range]`。
#[derive(Debug)]
pub(crate) struct WriteBatch {
    pub(crate) start_addr: u16,
    pub(crate) units: u16,
    pub(crate) range: std::ops::Range<usize>,
}

/// 把「(地址, 单元数)」列表合并为若干写片段（**严格连续**规则）。
///
/// 与读合并（允许空洞 ≤ gap）不同：FC15/FC16 写帧会覆盖 `[start, start + units)` 内的
/// **全部**地址，若把中间存在空洞的点位并入同一帧，会误写空洞地址上原有的线圈/寄存器。
/// 因此仅当 `下一项地址 == 当前片段末尾地址` 时才合并，出现空洞必须拆帧。
///
/// `items` 必须已按地址升序排列（由调用方保证）。
/// 合并后单元数不得超过 `max_units`，超限时另起新片段（超帧上限回退）。
pub(crate) fn merge_write_batches(items: &[(u16, u16)], max_units: u16) -> Vec<WriteBatch> {
    let mut batches: Vec<WriteBatch> = Vec::new();
    for (i, &(addr, units)) in items.iter().enumerate() {
        if let Some(last) = batches.last_mut() {
            let end = last.start_addr.saturating_add(last.units);
            let merged_units = last.units.saturating_add(units);
            if addr == end && merged_units <= max_units {
                last.units = merged_units;
                last.range.end = i + 1;
                continue;
            }
        }
        batches.push(WriteBatch {
            start_addr: addr,
            units,
            range: i..i + 1,
        });
    }
    batches
}

/// 把工程量的原始值 `raw` 按点位数据类型 / 字节序编码为 FC16 待写的 16 位寄存器序列。
///
/// 编码是 `parse_point_value`（解码）的逆过程：
/// 1. 先按数据类型写出**大端**原始字节（与 `decode_binary_value` 对称）；
/// 2. 再经 `reorder_bytes`（自逆置换）转换为线上字节序；
/// 3. 两两拼成寄存器（大端）。
///
/// 单寄存器类型（U16/I16，仅 2 字节）跳过 `reorder_bytes`，避免其 `len < 4` 告警刷屏。
pub(crate) fn encode_register_values(point: &PointConfig, raw: f64) -> Vec<u16> {
    let dt = point.data_type_enum().unwrap_or(PointDataType::U16);
    let be: Vec<u8> = match dt {
        PointDataType::U16 => (raw as u16).to_be_bytes().to_vec(),
        PointDataType::I16 => (raw as i16).to_be_bytes().to_vec(),
        PointDataType::U32 => (raw as u32).to_be_bytes().to_vec(),
        PointDataType::I32 => (raw as i32).to_be_bytes().to_vec(),
        PointDataType::F32 => (raw as f32).to_be_bytes().to_vec(),
        PointDataType::U64 => (raw as u64).to_be_bytes().to_vec(),
        PointDataType::I64 => (raw as i64).to_be_bytes().to_vec(),
        PointDataType::F64 => raw.to_be_bytes().to_vec(),
    };
    let wire = if be.len() >= 4 {
        reorder_bytes(&be, point.byte_order_enum())
    } else {
        be
    };
    let mut regs = Vec::with_capacity(wire.len() / 2);
    for i in 0..wire.len() / 2 {
        regs.push(u16::from_be_bytes([wire[i * 2], wire[i * 2 + 1]]));
    }
    regs
}
