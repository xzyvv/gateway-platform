//! Modbus 采集器的 `Collector` trait 实现：单点读写 / 批量读 / 多点写
//!
//! 固有方法（构造、重试读、批量读）见 `collector.rs`；写分组辅助见 `merge.rs`。

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use crate::config::PointConfig;
use crate::engine::codec::crc::crc16;
use crate::engine::model::{DataPoint, Quality};

use super::merge::WriteItem;
use super::{
    build_read_request, build_write_multi_coils, build_write_multi_registers,
    clone_collector_error, convert_value, encode_register_values, merge_write_batches,
    parse_point_value, parse_read_response, reverse_convert, write_kind_of, Collector,
    CollectorError, CollectorResult, ModbusCollector, ModbusExceptionCode, WriteKind,
    MAX_WRITE_COILS, MAX_WRITE_REGISTERS,
};

impl Collector for ModbusCollector {
    fn name(&self) -> &'static str {
        "modbus"
    }

    fn bus_name(&self) -> &str {
        self.bus.name()
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// C2：Modbus 族诊断 = 建链后用 FC03 真实读一次保持寄存器。
    ///
    /// 探针固定 `slave=1 / addr=0 / count=1`：不依赖点位配置，单帧最小开销，
    /// 也是绝大多数 Modbus 兼容设备的通用可读区起点。回读成功即同时证明
    /// 「链路可用 + 从站应答 + 帧格式/CRC 正确」；异常码与无应答按真实原因分档
    /// 报告，绝不返回假 "ok"（假成功会让运维把设备故障读成链路正常）。
    fn diag<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            const DIAG_SLAVE: u8 = 1;
            const DIAG_ADDR: u16 = 0;
            const DIAG_COUNT: u16 = 1;
            let bus = self.bus.name().to_string();
            let request = build_read_request(DIAG_SLAVE, 0x03, DIAG_ADDR, DIAG_COUNT);

            let response = match self.bus.transaction(request, Some(DIAG_COUNT)).await {
                Ok(r) => r,
                Err(CollectorError::ModbusException(code)) => {
                    let exc = ModbusExceptionCode::from_code(code);
                    return format!(
                        "modbus@{}: 链路可达，但从站 {} 对 FC03@{}+1 返回异常码 0x{:02X}（{}）；核对从站号/寄存器地址是否对应现场设备",
                        bus, DIAG_SLAVE, DIAG_ADDR, code, exc.description()
                    );
                }
                Err(e) => {
                    return format!(
                        "modbus@{}: FC03 读取无应答/链路失败（{}）；检查总线端点（串口/TCP）、波特率/校验位、从站供电与接线、超时设置",
                        bus, e
                    );
                }
            };

            match parse_read_response(DIAG_SLAVE, 0x03, DIAG_COUNT, &response) {
                Ok(regs) => format!(
                    "modbus@{}: FC03 读取成功（slave={}, 起始地址={}, 寄存器数={}, 回读值={}）",
                    bus,
                    DIAG_SLAVE,
                    DIAG_ADDR,
                    regs.len(),
                    regs.first().copied().unwrap_or(0)
                ),
                Err(CollectorError::ModbusException(code)) => {
                    let exc = ModbusExceptionCode::from_code(code);
                    format!(
                        "modbus@{}: 链路可达，但从站 {} 对 FC03@{}+1 返回异常码 0x{:02X}（{}）；核对从站号/寄存器地址是否对应现场设备",
                        bus, DIAG_SLAVE, DIAG_ADDR, code, exc.description()
                    )
                }
                Err(e) => format!(
                    "modbus@{}: 回帧校验失败（{}）；设备有应答但帧不合法，检查校验位/字节序/总线干扰",
                    bus, e
                ),
            }
        })
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>> {
        Box::pin(async move {
            let fc = point.function_code.unwrap_or(3);
            if fc == 1 || fc == 2 {
                // Bug 186 (N16): single-point read uses _with_retry variant.
                let slave = point.slave_id.unwrap_or(1);
                let addr = point.address_num.unwrap_or(0);
                let bits = self.read_coils_with_retry(slave, fc, addr, 1).await?;
                let bit = bits.first().copied().unwrap_or(false);
                let raw = if bit { 1.0 } else { 0.0 };
                let value = convert_value(point, raw);
                return Ok(DataPoint {
                    point_id: point.id.clone(),
                    device_id: point.device_id.clone().unwrap_or_else(|| point.bus.clone()),
                    value,
                    raw_value: raw,
                    raw_type: "bool".to_string(),
                    quality: Quality::Good,
                    quality_reason: String::new(),
                    timestamp: DataPoint::now(),
                    source_ts: DataPoint::now(),
                    stale_count: 0,
                    value_text: None,
                });
            }

            let reg_count = point.register_count.unwrap_or_else(|| {
                point
                    .data_type_enum()
                    .map(|d| d.register_count())
                    .unwrap_or(1)
            });
            let slave = point.slave_id.unwrap_or(1);
            let fc = point.function_code.unwrap_or(3);
            let addr = point.address_num.unwrap_or(0);
            // Bug 186 (N16): single-point read uses _with_retry variant.
            let regs = self
                .read_registers_with_retry(slave, fc, addr, reg_count)
                .await?;
            Ok(parse_point_value(point, &regs, addr))
        })
    }

    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> Pin<Box<dyn Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>> {
        Box::pin(async move {
            self.read_batch_points(points)
                .await
                .into_iter()
                .map(Ok)
                .collect()
        })
    }

    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let slave = point.slave_id.unwrap_or(1);
            let fc = point.function_code.unwrap_or(3);
            let addr = point.address_num.unwrap_or(0);

            // 反向换算：value -> raw（公式无法反推时直接取 value 作为 raw）
            let raw = reverse_convert(point, value);

            // Bug 161: 按配置功能码映射到正确的“写”功能码。
            //   1 | 5  -> 0x05  写单线圈
            //   3 | 6  -> 0x06  写单寄存器
            //   15      -> 0x0F  写多线圈（FC15；当前实现保守用 count=1，等价于 FC05）
            //   16      -> 0x10  写多寄存器（FC16；当前取 value 作为唯一待写寄存器）
            // 旧实现把所有非 1/5 的 fc 一律当作 FC06 写寄存器，FC04/FC02 等只读点。
            // 会被写到错误的寄存器/线圈空间。FC02/FC04 为只读，显式拒绝。
            let write_fc: u8 = match fc {
                1 | 5 => 0x05,
                3 | 6 => 0x06,
                15 => 0x0F,
                16 => 0x10,
                other => {
                    return Err(CollectorError::InvalidFrame(format!(
                        "unsupported write function code {} (only fc 1/5/3/6/15/16 are writable)",
                        other
                    )));
                }
            };

            let req: Vec<u8> = match write_fc {
                0x05 => {
                    // 写单个线圈 FC05
                    let coil_val = if raw != 0.0 { 0xFF } else { 0x00 };
                    let mut frame = vec![
                        slave,
                        0x05,
                        (addr >> 8) as u8,
                        (addr & 0xFF) as u8,
                        coil_val,
                        0x00,
                    ];
                    let crc = crc16(&frame);
                    frame.push((crc & 0xFF) as u8);
                    frame.push((crc >> 8) as u8);
                    frame
                }
                0x06 => {
                    // Bug 219: 旧实现用 `raw as u16` 浮点转整型走饱和语义——
                    // I16 点位写负值（如 -5）会饱和到 0，写入值错误。
                    // 改用 encode_register_values 按数据类型编码（I16 走
                    // (raw as i16).to_be_bytes()），与读路径解码互逆。
                    // Bug 220: 多寄存器类型（U32/I32/F32 等）FC06 一帧写不下。
                    // 旧实现只写低 1 个寄存器导致值损坏；regs 多于 1 个时
                    // 自动升级到 FC16 写全部寄存器。
                    let regs = encode_register_values(point, raw);
                    if regs.len() == 1 {
                        // 写单个寄存器 FC06
                        let reg_val = regs[0];
                        let mut frame = vec![
                            slave,
                            0x06,
                            (addr >> 8) as u8,
                            (addr & 0xFF) as u8,
                            (reg_val >> 8) as u8,
                            (reg_val & 0xFF) as u8,
                        ];
                        let crc = crc16(&frame);
                        frame.push((crc & 0xFF) as u8);
                        frame.push((crc >> 8) as u8);
                        frame
                    } else {
                        build_write_multi_registers(slave, addr, &regs)
                    }
                }
                0x0F => {
                    // 写多线圈 FC15：本轮保守实现用 count=1（单线圈），
                    // 走与 FC05 等价的语义。后续如需"真多线圈批量"，
                    // 需要扩展 Collector::write 接受线圈位数组，超出本轮范围。
                    let coils = [raw != 0.0];
                    build_write_multi_coils(slave, addr, 1, &coils)
                }
                0x10 => {
                    // Bug 220: 旧实现固定只写 1 个寄存器（[raw as u16]），
                    // U32/I32/F32 占 2 寄存器（I64 占 4 寄存器）类型只写一半，
                    // 从站侧值损坏。按数据类型编码全部寄存器后整帧下发。
                    let regs = encode_register_values(point, raw);
                    build_write_multi_registers(slave, addr, &regs)
                }
                _ => unreachable!("write_fc mapped above"),
            };

            // Bug 161: 正常响应是请求的 8 字节回显（slave,fc,addrH,addrL,valH,valL,crcl,crcH）。
            // 旧实现仅校验 len<6，6~7 字节的截断响应会被误判为写入成功。现要求 len>=8
            // 且逐字节与请求回显比对，防止截断/错配响应被当作成功。
            // Bug 218: FC15/FC16 的响应只有前 6 字节与请求相同（slave/fc/addr/qty），
            // 请求的第 6-7 字节是 byteCount+数据，而响应第 6-7 字节是 CRC——
            // 旧实现对所有写功能码统一做 8 字节全量回显比对，导致 FC15/FC16
            // 单点写必然失败（除非 CRC 恰好与数据字节相等）。现按帧内功能码区分。
            // FC05/FC06 保持 8 字节全回显比对；FC15/FC16 只比对前 6 字节
            // （与 write_multi 的多点写校验方式一致）。
            let expect_echo = req.clone();
            let resp = self.bus.transaction(req, None).await?;
            let echo_len = if matches!(expect_echo[1], 0x05 | 0x06) {
                8
            } else {
                6
            };
            if resp.len() < 8 || resp[..echo_len] != expect_echo[..echo_len] {
                return Err(CollectorError::InvalidFrame(
                    "write response mismatch (expected echo of request)".into(),
                ));
            }
            Ok(())
        })
    }

    /// 批量写入多个点位（决策表 #1：Modbus FC15/FC16 多点写）
    ///
    /// 分组策略。
    /// 1. 携带非法写功能码（`write_kind_of` 返回 `None`）的点位逐点回退到 `write`（返回具体错误）
    /// 2. 其余按 `(slave_id, WriteKind)` 分组（`BTreeMap` 保证遍历顺序稳定）
    /// 3. 组内按地址升序排列后，用 `merge_write_batches` 做**严格连续**合并（空洞必须拆帧）
    /// 4. 单点片段仍走 `write`（保持 FC05/FC06 语义与 8 字节回显校验）；
    ///    多点片段构建 FC15/FC16 帧，一次事务写多个线圈/寄存器。
    ///
    /// 返回结果与入参**等长且逐位对应**（按下标回填）。
    fn write_multi<'a>(
        &'a self,
        points: &'a [(&'a PointConfig, f64)],
    ) -> Pin<Box<dyn Future<Output = Vec<CollectorResult<()>>> + Send + 'a>> {
        Box::pin(async move {
            let n = points.len();
            let mut slots: Vec<Option<CollectorResult<()>>> = (0..n).map(|_| None).collect();

            // (slave, kind) -> 组内成员 [(addr, units, 入参下标)]
            let mut groups: BTreeMap<(u8, WriteKind), Vec<WriteItem>> = BTreeMap::new();
            for (idx, (point, _)) in points.iter().enumerate() {
                let fc = point.function_code.unwrap_or(3);
                match write_kind_of(fc) {
                    Some(kind) => {
                        let slave = point.slave_id.unwrap_or(1);
                        let addr = point.address_num.unwrap_or(0);
                        let units = if kind == WriteKind::Coil {
                            1
                        } else {
                            point.register_count.unwrap_or_else(|| {
                                point
                                    .data_type_enum()
                                    .map(|d| d.register_count())
                                    .unwrap_or(1)
                            })
                        };
                        groups
                            .entry((slave, kind))
                            .or_default()
                            .push((addr, units, idx));
                    }
                    // 非法写功能码：逐点回退，由 write 返回具体错误
                    None => {
                        slots[idx] = Some(self.write(points[idx].0, points[idx].1).await);
                    }
                }
            }

            for ((slave, kind), mut items) in groups {
                items.sort_by_key(|(addr, _, _)| *addr);
                let (max_units, is_coil) = match kind {
                    WriteKind::Coil => (MAX_WRITE_COILS, true),
                    WriteKind::Register => (MAX_WRITE_REGISTERS, false),
                };
                let keys: Vec<(u16, u16)> = items.iter().map(|(a, u, _)| (*a, *u)).collect();

                for batch in merge_write_batches(&keys, max_units) {
                    let range = batch.range.clone();
                    // 单点片段：回退到 write，保持 FC05/FC06 语义与严格 8 字节回显校验
                    if range.len() == 1 {
                        let idx = items[range.start].2;
                        slots[idx] = Some(self.write(points[idx].0, points[idx].1).await);
                        continue;
                    }

                    // 多点片段：构建 FC15/FC16 帧
                    let req = if is_coil {
                        let mut coils = Vec::with_capacity(batch.units as usize);
                        for i in range.clone() {
                            let idx = items[i].2;
                            let raw = reverse_convert(points[idx].0, points[idx].1);
                            coils.push(raw != 0.0);
                        }
                        build_write_multi_coils(slave, batch.start_addr, batch.units, &coils)
                    } else {
                        let mut regs = Vec::new();
                        for i in range.clone() {
                            let idx = items[i].2;
                            let (point, value) = points[idx];
                            let raw = reverse_convert(point, value);
                            regs.extend_from_slice(&encode_register_values(point, raw));
                        }
                        build_write_multi_registers(slave, batch.start_addr, &regs)
                    };

                    // FC15/FC16 响应 = 8 字节回显（slave,fc,addrH,addrL,qtyH,qtyL,crcL,crcH）。
                    // 请求的 6/7 字节是 byte_count + 数据，与响应第 6/7 字节（CRC）不同，
                    // 故多帧写只能比对**前 6 字节**（slave/fc/addr/qty），不能整段比对。
                    let expect = req.clone();
                    let resp = match self.bus.transaction(req, None).await {
                        Ok(r) => r,
                        Err(e) => {
                            for i in range.clone() {
                                slots[items[i].2] = Some(Err(clone_collector_error(&e)));
                            }
                            continue;
                        }
                    };
                    let echo_len = 6.min(expect.len());
                    if resp.len() < 8 || resp[..echo_len] != expect[..echo_len] {
                        let msg = format!(
                            "write_multi response mismatch (slave {}, addr {}, expected {}-byte echo)",
                            slave,
                            batch.start_addr,
                            expect.len()
                        );
                        for i in range.clone() {
                            slots[items[i].2] =
                                Some(Err(CollectorError::InvalidFrame(msg.clone())));
                        }
                        continue;
                    }
                    for i in range.clone() {
                        slots[items[i].2] = Some(Ok(()));
                    }
                }
            }

            slots
                .into_iter()
                .map(|s| {
                    s.unwrap_or_else(|| {
                        Err(CollectorError::InvalidFrame(
                            "modbus write_multi internal: point not processed".into(),
                        ))
                    })
                })
                .collect()
        })
    }
}
