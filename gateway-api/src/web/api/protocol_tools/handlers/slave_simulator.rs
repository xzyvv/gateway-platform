//! 从站模拟器 handler（真实 Modbus TCP 从站）
//!
//! **能力**：启动后在网关本机 `0.0.0.0:<port>` 监听 TCP，作为 Modbus TCP 从站供
//! 外部主站连接并读写寄存器。寄存器值与 `GET /simulator/registers` 是同一份内存
//! 数据——主站写入后页面上立即可见。
//!
//! **功能码支持**：
//! - `0x03` 读保持寄存器、`0x04` 读输入寄存器（两者共用同一份寄存器表）
//! - `0x06` 写单个寄存器、`0x10` 写多个寄存器
//! - `0x01` 读线圈 / `0x02` 读离散量：按「寄存器值的 bit0」映射
//!   （线圈地址 == 寄存器地址，1 个寄存器提供 1 个 bit，非 0 即 ON）
//! - 其它功能码 → 异常 `0x01`（非法功能）；地址不在寄存器表 → `0x02`（非法数据
//!   地址）；数量越界 → `0x03`（非法数据值）
//!
//! **边界（如实声明）**：仅 Modbus TCP（MBAP），**不含 RTU/ASCII 串口从站模拟**；
//! 寄存器为进程内内存态、不落盘，进程重启回到 6 条默认寄存器；监听器随进程退出，
//! 不做开机自启。`unit_id` 与启动时配置的 `slave_id` 不一致时按 Modbus TCP 惯例
//! 不应答（多从站共用端口时避免抢答，由主站自行超时重试；广播地址同样不应答）。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::Json;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use ts_rs::TS;

use crate::web::types::{ApiError, ApiResponse, AppState};

/// 默认监听端口（前端表单默认值同此）
const DEFAULT_PORT: u16 = 1502;

/// 模拟寄存器
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SimRegister {
    pub addr: i32,
    pub name: String,
    pub value: i32,
    #[serde(rename = "type")]
    pub reg_type: String,
    pub unit: Option<String>,
}

/// 模拟器状态
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SimulatorStatus {
    pub running: bool,
    pub slave_id: Option<i32>,
    /// 当前（或最近一次）监听端口；`port=0` 启动时为系统实际分配的端口
    pub port: u16,
    /// 运行中的监听地址（如 `0.0.0.0:1502`）；停止后为 `null`
    pub listen_addr: Option<String>,
    /// 已处理的 Modbus TCP 请求数（含异常与 unit_id 不匹配的请求）
    #[ts(type = "number")]
    pub requests: u64,
    /// 解析失败 / 返回异常响应的次数
    #[ts(type = "number")]
    pub errors: u64,
    /// 最近一次启动时间（毫秒时间戳）；停止后为 `null`
    #[ts(type = "number")]
    pub started_at_ms: Option<u64>,
}

#[derive(Deserialize)]
pub struct UpdateRegBody {
    pub value: i32,
}

#[derive(Deserialize)]
pub struct StartSimBody {
    pub slave_id: i32,
    /// 监听端口，缺省 1502；传 0 表示由系统分配（测试用）
    #[serde(default)]
    pub port: Option<u16>,
}

struct SimState {
    registers: Vec<SimRegister>,
    running: bool,
    slave_id: Option<i32>,
    port: u16,
    listen_addr: Option<String>,
    requests: u64,
    errors: u64,
    started_at_ms: Option<u64>,
    /// 后台监听任务的句柄；stop 时 abort 并等待其退出（确保端口立即释放）
    task: Option<tokio::task::JoinHandle<()>>,
}

fn default_regs() -> Vec<SimRegister> {
    vec![
        SimRegister {
            addr: 0,
            name: "模拟电压".into(),
            value: 220,
            reg_type: "uint16".into(),
            unit: Some("V".into()),
        },
        SimRegister {
            addr: 1,
            name: "模拟电流".into(),
            value: 5,
            reg_type: "uint16".into(),
            unit: Some("A".into()),
        },
        SimRegister {
            addr: 2,
            name: "模拟功率".into(),
            value: 1100,
            reg_type: "uint16".into(),
            unit: Some("W".into()),
        },
        SimRegister {
            addr: 3,
            name: "模拟频率".into(),
            value: 50,
            reg_type: "uint16".into(),
            unit: Some("Hz".into()),
        },
        SimRegister {
            addr: 10,
            name: "状态字".into(),
            value: 0,
            reg_type: "uint16".into(),
            unit: None,
        },
        SimRegister {
            addr: 11,
            name: "故障码".into(),
            value: 0,
            reg_type: "uint16".into(),
            unit: None,
        },
    ]
}

/// 进程内模拟器状态（**临时态**：不落盘，重启回到默认寄存器，
/// 口径见 `docs/api-protocol-tools.md` §3）
static SIM: Lazy<Mutex<SimState>> = Lazy::new(|| {
    Mutex::new(SimState {
        registers: default_regs(),
        running: false,
        slave_id: None,
        port: DEFAULT_PORT,
        listen_addr: None,
        requests: 0,
        errors: 0,
        started_at_ms: None,
        task: None,
    })
});

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 状态快照（handler 与测试共用；持锁时间仅限拷贝字段）
fn status_snapshot() -> SimulatorStatus {
    let s = SIM.lock();
    SimulatorStatus {
        running: s.running,
        slave_id: s.slave_id,
        port: s.port,
        listen_addr: s.listen_addr.clone(),
        requests: s.requests,
        errors: s.errors,
        started_at_ms: s.started_at_ms,
    }
}

pub(crate) async fn list_sim_registers(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<SimRegister>>>, ApiError> {
    let s = SIM.lock();
    Ok(Json(ApiResponse::ok(s.registers.clone())))
}

pub(crate) async fn update_sim_register(
    State(_state): State<Arc<AppState>>,
    Path(addr): Path<i32>,
    Json(body): Json<UpdateRegBody>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let mut s = SIM.lock();
    if let Some(r) = s.registers.iter_mut().find(|r| r.addr == addr) {
        r.value = body.value;
    } else {
        return Err(ApiError::not_found(format!("register not found: {}", addr)));
    }
    Ok(Json(ApiResponse::ok(())))
}

pub(crate) async fn start_simulator(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<StartSimBody>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    start_impl(body.slave_id, body.port).await?;
    Ok(Json(ApiResponse::ok(())))
}

pub(crate) async fn stop_simulator(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    stop_impl().await;
    Ok(Json(ApiResponse::ok(())))
}

pub(crate) async fn get_simulator_status(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<SimulatorStatus>>, ApiError> {
    Ok(Json(ApiResponse::ok(status_snapshot())))
}

/// 启动 Modbus TCP 从站。返回实际监听端口（`port=0` 时为系统分配值）。
async fn start_impl(slave_id: i32, port: Option<u16>) -> Result<u16, ApiError> {
    if SIM.lock().running {
        return Err(ApiError::bad_request(
            "从站模拟器已在运行，请先停止后再启动",
        ));
    }
    let want = port.unwrap_or(DEFAULT_PORT);
    let listener = TcpListener::bind(("0.0.0.0", want)).await.map_err(|e| {
        ApiError::bad_request(format!(
            "监听 0.0.0.0:{} 失败（端口可能已被占用）：{}",
            want, e
        ))
    })?;
    let local = listener
        .local_addr()
        .map_err(|e| ApiError::internal(format!("读取监听地址失败：{}", e)))?;
    let actual_port = local.port();
    let listen_addr = local.to_string();

    let task = tokio::spawn(run_server(listener));

    let mut s = SIM.lock();
    s.running = true;
    s.slave_id = Some(slave_id);
    s.port = actual_port;
    s.listen_addr = Some(listen_addr);
    s.requests = 0;
    s.errors = 0;
    s.started_at_ms = Some(now_ms());
    s.task = Some(task);
    Ok(actual_port)
}

/// 停止监听：先置标记再 abort 并等待后台任务退出，保证端口立即释放。
async fn stop_impl() {
    let task = {
        let mut s = SIM.lock();
        s.running = false;
        s.slave_id = None;
        s.listen_addr = None;
        s.started_at_ms = None;
        s.task.take()
    };
    if let Some(task) = task {
        task.abort();
        let _ = task.await;
    }
}

/// 接受连接的后台任务。连接交给 `JoinSet` 并发处理；主任务被 abort 时
/// `JoinSet` 随局部变量析构，其子任务（含已建立的连接）一并中止。
async fn run_server(listener: TcpListener) {
    let mut conns: JoinSet<()> = JoinSet::new();
    loop {
        match listener.accept().await {
            Ok((stream, _peer)) => {
                conns.spawn(handle_connection(stream));
                // 顺手回收已结束的连接任务，避免结果在集合中长期堆积
                while conns.try_join_next().is_some() {}
            }
            Err(_) => {
                // accept 失败通常是瞬时资源问题，稍作退避后继续
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// 处理一条 TCP 连接：循环读取 MBAP 帧 → 应答，直到对端关闭或帧边界不可信。
async fn handle_connection(mut stream: TcpStream) {
    loop {
        let mut header = [0u8; 7];
        if stream.read_exact(&mut header).await.is_err() {
            return;
        }
        let tid = u16::from_be_bytes([header[0], header[1]]);
        let protocol = u16::from_be_bytes([header[2], header[3]]);
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;
        let unit = header[6];

        // 协议号必须为 0；长度 = unit_id(1) + PDU，至少 2。非法帧无法可靠切分后续
        // 数据流，计数后直接断开连接（与 Modbus TCP 实现的常见处置一致）。
        if protocol != 0 || !(2..=260).contains(&length) {
            SIM.lock().errors += 1;
            return;
        }

        let mut pdu = vec![0u8; length - 1];
        if stream.read_exact(&mut pdu).await.is_err() {
            return;
        }

        let (running, slave_id) = {
            let mut s = SIM.lock();
            if s.running {
                s.requests += 1;
            }
            (s.running, s.slave_id)
        };
        if !running {
            return;
        }
        let Some(slave_id) = slave_id else {
            return;
        };

        // unit_id 必须等于配置的从站地址。不匹配时不应答：TCP 端口上可能被多个
        // 从站共享，抢答会让主站把错误从站的数据当成目标从站的响应。
        if unit as i32 != slave_id {
            continue;
        }

        let resp_pdu = process_pdu(&pdu);
        let frame = encode_mbap(tid, unit, &resp_pdu);
        if stream.write_all(&frame).await.is_err() {
            return;
        }
    }
}

/// 组装 MBAP 响应头 + PDU
fn encode_mbap(tid: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
    let length = (pdu.len() + 1) as u16; // unit_id + PDU
    let mut out = Vec::with_capacity(7 + pdu.len());
    out.extend_from_slice(&tid.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    out.push(unit);
    out.extend_from_slice(pdu);
    out
}

fn exception_pdu(fc: u8, code: u8) -> Vec<u8> {
    vec![fc | 0x80, code]
}

/// 按 PDU 生成响应 PDU。寄存器读写与请求计数在同一把锁内完成（函数为同步，
/// 不跨 await 持锁），因此主站写入能立即反映到 `GET /simulator/registers`。
fn process_pdu(pdu: &[u8]) -> Vec<u8> {
    let fc = pdu[0]; // MBAP 长度校验已保证 PDU 至少 1 字节
    let mut s = SIM.lock();

    match fc {
        // 读保持寄存器 / 读输入寄存器（共用同一份寄存器表）
        0x03 | 0x04 => {
            if pdu.len() != 5 {
                s.errors += 1;
                return exception_pdu(fc, 0x03);
            }
            let start = u16::from_be_bytes([pdu[1], pdu[2]]) as usize;
            let qty = u16::from_be_bytes([pdu[3], pdu[4]]) as usize;
            if qty == 0 || qty > 125 {
                s.errors += 1;
                return exception_pdu(fc, 0x03);
            }
            let end = start + qty;
            if end > 0x1_0000 {
                s.errors += 1;
                return exception_pdu(fc, 0x02);
            }
            let mut values = Vec::with_capacity(qty);
            for addr in start..end {
                // 用 position 取下标而非 find+expect：缺失时直接返回异常码，
                // 无需先 any() 再 find() 双遍历，也不引入 panic 点。
                match s.registers.iter().position(|r| r.addr == addr as i32) {
                    Some(idx) => {
                        let v = s.registers[idx].value;
                        values.push((v as u32 & 0xFFFF) as u16);
                    }
                    None => {
                        s.errors += 1;
                        return exception_pdu(fc, 0x02);
                    }
                }
            }
            let mut out = Vec::with_capacity(2 + qty * 2);
            out.push(fc);
            out.push((qty * 2) as u8);
            for v in values {
                out.extend_from_slice(&v.to_be_bytes());
            }
            out
        }
        // 写单个寄存器
        0x06 => {
            if pdu.len() != 5 {
                s.errors += 1;
                return exception_pdu(fc, 0x03);
            }
            let addr = u16::from_be_bytes([pdu[1], pdu[2]]) as i32;
            let value = u16::from_be_bytes([pdu[3], pdu[4]]) as i32;
            if !s.registers.iter().any(|r| r.addr == addr) {
                s.errors += 1;
                return exception_pdu(fc, 0x02);
            }
            if let Some(r) = s.registers.iter_mut().find(|r| r.addr == addr) {
                r.value = value;
            }
            pdu.to_vec() // 正常响应回显请求
        }
        // 写多个寄存器
        0x10 => {
            if pdu.len() < 6 {
                s.errors += 1;
                return exception_pdu(fc, 0x03);
            }
            let start = u16::from_be_bytes([pdu[1], pdu[2]]) as usize;
            let qty = u16::from_be_bytes([pdu[3], pdu[4]]) as usize;
            let byte_count = pdu[5] as usize;
            if qty == 0 || qty > 123 || byte_count != qty * 2 || pdu.len() != 6 + byte_count {
                s.errors += 1;
                return exception_pdu(fc, 0x03);
            }
            let end = start + qty;
            if end > 0x1_0000 {
                s.errors += 1;
                return exception_pdu(fc, 0x02);
            }
            // 先整体校验地址，避免部分写入（Modbus 语义：要么全成功要么返回异常）
            for addr in start..end {
                if !s.registers.iter().any(|r| r.addr == addr as i32) {
                    s.errors += 1;
                    return exception_pdu(fc, 0x02);
                }
            }
            for (i, addr) in (start..end).enumerate() {
                let raw = u16::from_be_bytes([pdu[6 + i * 2], pdu[7 + i * 2]]) as i32;
                if let Some(r) = s.registers.iter_mut().find(|r| r.addr == addr as i32) {
                    r.value = raw;
                }
            }
            vec![0x10, pdu[1], pdu[2], pdu[3], pdu[4]]
        }
        // 读线圈 / 读离散量：寄存器值的 bit0 即开关量（地址与寄存器地址一一对应）
        0x01 | 0x02 => {
            if pdu.len() != 5 {
                s.errors += 1;
                return exception_pdu(fc, 0x03);
            }
            let start = u16::from_be_bytes([pdu[1], pdu[2]]) as usize;
            let qty = u16::from_be_bytes([pdu[3], pdu[4]]) as usize;
            if qty == 0 || qty > 2000 {
                s.errors += 1;
                return exception_pdu(fc, 0x03);
            }
            let end = start + qty;
            if end > 0x1_0000 {
                s.errors += 1;
                return exception_pdu(fc, 0x02);
            }
            let mut bits = vec![0u8; qty.div_ceil(8)];
            for (i, addr) in (start..end).enumerate() {
                // 同 coil 分支：position 单遍定位，缺寄存器即返回异常码，避免
                // any()+find()+expect 的三次遍历与 panic 点。
                match s.registers.iter().position(|r| r.addr == addr as i32) {
                    Some(idx) if s.registers[idx].value & 1 != 0 => {
                        bits[i / 8] |= 1 << (i % 8);
                    }
                    Some(_) => {}
                    None => {
                        s.errors += 1;
                        return exception_pdu(fc, 0x02);
                    }
                }
            }
            let mut out = Vec::with_capacity(2 + bits.len());
            out.push(fc);
            out.push(bits.len() as u8);
            out.extend_from_slice(&bits);
            out
        }
        // 其它功能码：非法功能
        _ => {
            s.errors += 1;
            exception_pdu(fc, 0x01)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 静态 SIM 是全局状态：用一把异步锁把用例串行化，避免端口/寄存器互相干扰。
    static TEST_GUARD: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

    async fn start(slave_id: i32, port: Option<u16>) -> u16 {
        start_impl(slave_id, port)
            .await
            .unwrap_or_else(|e| panic!("start simulator failed: {}", e.message))
    }

    async fn reset() {
        stop_impl().await;
        let mut s = SIM.lock();
        s.registers = default_regs();
        s.running = false;
        s.slave_id = None;
        s.port = DEFAULT_PORT;
        s.listen_addr = None;
        s.requests = 0;
        s.errors = 0;
        s.started_at_ms = None;
        s.task = None;
    }

    /// 组装 MBAP 帧（PDU 由调用方构造）
    fn frame(tid: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
        encode_mbap(tid, unit, pdu)
    }

    /// 建立连接、发送一帧、按 MBAP 长度读回完整响应
    async fn transact(port: u16, req: &[u8]) -> Vec<u8> {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect simulator");
        stream.write_all(req).await.expect("write request");
        let mut header = [0u8; 7];
        stream
            .read_exact(&mut header)
            .await
            .expect("read response header");
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;
        let mut body = vec![0u8; length - 1];
        stream
            .read_exact(&mut body)
            .await
            .expect("read response body");
        let mut resp = header.to_vec();
        resp.extend_from_slice(&body);
        resp
    }

    #[tokio::test]
    async fn start_with_port_zero_serves_fc03_read() {
        let _guard = TEST_GUARD.lock().await;
        reset().await;

        let port = start(1, Some(0)).await;
        let st = status_snapshot();
        assert!(st.running);
        assert_eq!(st.port, port);
        assert_eq!(
            st.listen_addr.as_deref(),
            Some(format!("0.0.0.0:{}", port).as_str())
        );
        assert_eq!(st.slave_id, Some(1));

        // FC03：事务号 0x1234，从地址 0 读 2 个寄存器（220、5）
        let req = frame(0x1234, 1, &[0x03, 0x00, 0x00, 0x00, 0x02]);
        let resp = transact(port, &req).await;

        assert_eq!(&resp[0..2], &[0x12, 0x34]); // 事务号原样回填
        assert_eq!(&resp[2..4], &[0x00, 0x00]); // 协议号 0
        assert_eq!(u16::from_be_bytes([resp[4], resp[5]]), 7); // unit + PDU
        assert_eq!(resp[6], 1); // 单元号
        assert_eq!(resp[7], 0x03);
        assert_eq!(resp[8], 4); // 字节数
        assert_eq!(&resp[9..11], &[0x00, 0xDC]); // 220
        assert_eq!(&resp[11..13], &[0x00, 0x05]); // 5

        assert_eq!(status_snapshot().requests, 1);
        stop_impl().await;
    }

    #[tokio::test]
    async fn fc06_write_updates_register_table() {
        let _guard = TEST_GUARD.lock().await;
        reset().await;

        let port = start(1, Some(0)).await;
        // FC06：把地址 0 写为 500（0x01F4）
        let req = frame(0x0001, 1, &[0x06, 0x00, 0x00, 0x01, 0xF4]);
        let resp = transact(port, &req).await;

        assert_eq!(resp[7], 0x06); // 正常响应回显功能码
        assert_eq!(&resp[8..12], &[0x00, 0x00, 0x01, 0xF4]);

        let regs = { SIM.lock().registers.clone() };
        let reg0 = regs.iter().find(|r| r.addr == 0).expect("register 0");
        assert_eq!(reg0.value, 500);

        stop_impl().await;
    }

    #[tokio::test]
    async fn unsupported_function_returns_exception_01() {
        let _guard = TEST_GUARD.lock().await;
        reset().await;

        let port = start(1, Some(0)).await;
        // 0x05（写单个线圈）未实现 → 异常 0x85 + 0x01
        let req = frame(0x0002, 1, &[0x05, 0x00, 0x00, 0xFF, 0x00]);
        let resp = transact(port, &req).await;

        assert_eq!(u16::from_be_bytes([resp[4], resp[5]]), 3); // unit + fc + code
        assert_eq!(resp[7], 0x85);
        assert_eq!(resp[8], 0x01);
        assert_eq!(status_snapshot().errors, 1);

        stop_impl().await;
    }

    #[tokio::test]
    async fn stop_closes_listen_port() {
        let _guard = TEST_GUARD.lock().await;
        reset().await;

        let port = start(1, Some(0)).await;
        // 先确认端口可连
        TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect before stop");

        stop_impl().await;
        assert!(!status_snapshot().running);
        assert!(status_snapshot().listen_addr.is_none());

        // 停止后端口已释放，连接应被拒绝
        let err = TcpStream::connect(("127.0.0.1", port)).await;
        assert!(err.is_err(), "port should be closed after stop");
    }
}
