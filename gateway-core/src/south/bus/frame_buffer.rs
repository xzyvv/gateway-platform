//! 串口分帧缓冲层（§8.3 稳定性保障 #7）
//!
//! 按协议分隔符切分完整帧，避免半包/粘包。
//! - DLT645：0x68 ... 0x68 ... 0x16
//! - CJT188：0x68 ... 0x16
//! - 通用：按长度或分隔符
//!
//! C-10 修复：
//! 旧实现硬上限是 `max_len * 2`，截断策略目标 `max_len`，但兜底分支又
//! 把硬上限放宽到 `hard_cap = max_len * 2`——单一不变量不存在：实际保留
//! 字节数在 `[max_len, 2 * max_len]` 之间抖动，取决于帧头位置。
//! 现场电磁干扰下两帧头若恰好靠近，缓冲可能持续接近 `2 × max_len` 才截断，
//! 与「防止内存膨胀」的设计目标不符。
//!
//! 新设计：
//! 1. **单一不变量** `buf.len() <= max_len` —— 每次 `push` 末尾硬截断保证
//! 2. 截断策略：保留「最近 1 个帧头（0x68/0x55）到结尾」的数据
//!    （单点保留比「最近 2 个帧头之间」更稳定——两帧头靠近时不会退化）
//! 3. 找不到帧头时按 max_len 硬截断到末尾（保留最新数据）
//! 4. 溢出告警走 10 秒频率限制（Bug #41），避免电磁干扰场景日志刷屏

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 默认缓冲上限（4 倍单帧大小，足够容纳跨截断点的部分帧）
const DEFAULT_BUF_CAPACITY: usize = 4096;

/// 单帧最大字节数（防止恶意数据撑爆内存）
const MAX_FRAME_BYTES: usize = 2048;

/// C-10：溢出告警频率限制（毫秒）。同一进程 10s 内对同一缓冲只发 1 条 warn，
/// 避免电磁干扰场景下日志刷屏。
const OVERFLOW_WARN_INTERVAL_MS: u64 = 10_000;

/// C-10 修复：全局进程级「上次告警时间戳」。多个 FrameBuffer 共享同一原子，
/// 频率限制以「最近一次任一缓冲的告警」为锚——避免几十个总线同时溢出时
/// 每 10s 刷出几十条相同 warn。
static LAST_OVERFLOW_WARN_MS: AtomicU64 = AtomicU64::new(0);

/// 校验 DLT645 / CJT188 帧的校验和（CS）
///
/// CS = 帧首(0x68) 到 CS 前一字节所有字节之和 mod 256。
/// 两个协议帧头均为 10 字节（1+6+1+1+1 或 1+7+1+1），
/// CS 位于 data_end = 10 + data_len 处。
/// C-10 修复：通用分帧缓冲器（基于 VecDeque），严格保证 `len() <= max_len`。
#[derive(Debug, Clone)]
pub struct FrameBuffer {
    buf: VecDeque<u8>,
    max_len: usize,
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBuffer {
    /// 默认缓冲（4096 字节）
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BUF_CAPACITY)
    }

    /// 自定义缓冲上限
    pub fn with_capacity(max_len: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(max_len.min(MAX_FRAME_BYTES * 4)),
            max_len,
        }
    }

    /// 推入新数据
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend(data.iter().copied());
        self.enforce_limit();
    }

    /// C-10 修复：硬上限 + 截断策略
    ///
    /// 不变量：函数返回时 `self.buf.len() <= self.max_len`。
    /// 步骤：
    /// 1. 找到「最近 1 个帧头（0x68 或 0x55）」的位置
    /// 2. 截断到「该帧头到结尾」——单点保留策略比"保留 2 帧头之间"更稳定
    /// 3. 若仍超 `max_len`，**从帧头开始**只截前 `max_len` 字节（保留「最近
    ///    1 帧头起始 + 后续 max_len-1 字节」）——不能"丢帧头保留尾部纯数据"，
    ///    否则协议层无法识别这是一帧
    /// 4. 若无任何帧头，按 `max_len` 硬截到尾部（保留最新数据）
    /// 5. 溢出告警走 10s 频率限制
    fn enforce_limit(&mut self) {
        if self.buf.len() <= self.max_len {
            return;
        }

        // 步骤 1：找最近 1 个帧头位置（从尾向前扫）
        let last_header = self.buf.iter().rposition(|&b| b == 0x68 || b == 0x55);

        match last_header {
            Some(pos) => {
                // 步骤 2：截断到「最近帧头到结尾」
                if pos > 0 {
                    self.buf.drain(..pos);
                }
                // 步骤 3：若仍超 max_len，保留「最近 1 帧头 + 后续 max_len-1 字节」——
                // 从帧头位置开始只留 max_len 字节，丢末尾（绝不丢帧头起始）。
                if self.buf.len() > self.max_len {
                    self.buf.drain(self.max_len..);
                    // 此时 buf 长度恰好 = max_len；首字节 = 0x68/0x55（帧头）
                    debug_assert_eq!(self.buf.len(), self.max_len);
                }
            }
            None => {
                // 步骤 4：无帧头，硬截保留尾部 max_len 字节
                let drop = self.buf.len() - self.max_len;
                self.buf.drain(..drop);
            }
        }

        // 步骤 5：频率限制告警
        Self::warn_overflow_rate_limited();
    }

    /// 频率限制的溢出告警：10s 窗口内只 warn 一次（Bug #41）。
    /// 多 FrameBuffer 共享同一原子，避免几十个总线同时溢出时刷屏。
    fn warn_overflow_rate_limited() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_OVERFLOW_WARN_MS.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) < OVERFLOW_WARN_INTERVAL_MS {
            return;
        }
        // compare_exchange 保证多缓冲并发时只有一个真正写入并触发 warn
        if LAST_OVERFLOW_WARN_MS
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            tracing::warn!(
                "frame_buffer overflow detected (rate-limited to 1/{}s); \
                 next overflowing push within window will be silent. \
                 check upstream bus/serial health if this repeats.",
                OVERFLOW_WARN_INTERVAL_MS / 1000
            );
        }
    }

    /// 当前缓冲长度
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// 查看当前缓冲内容（不移除）
    ///
    /// Bug 61 修复：原实现 `as_slices().0` 只返回 VecDeque 第一段连续切片，
    /// 当 buf 被多次 push/pop 形成多段时，会丢失中间或末尾数据。
    /// 改为返回完整连续副本（与 peek_all 一致，但避免重复分配）。
    pub fn peek(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    /// 查看全部缓冲内容（不移除），返回 Vec 副本
    ///
    /// 与 `peek()` 不同，此方法返回连续的 Vec，不受 VecDeque 内部分段影响。
    /// 用于协议帧解析（DLT645/CJT188）需要完整缓冲的场景。
    pub fn peek_all(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    /// 弹出指定长度数据
    pub fn pop(&mut self, n: usize) -> Vec<u8> {
        let n = n.min(self.buf.len());
        self.buf.drain(..n).collect()
    }

    /// 清空缓冲
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests;
