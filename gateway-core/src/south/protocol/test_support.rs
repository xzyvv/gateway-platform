//! 协议帧性质测试的确定性伪随机源（仅测试构建）。
//!
//! 为什么不引 `rand` / `proptest`：仓库已有 CJT188 的 LCG 范式，固定种子让失败
//! 可逐字节复现，且不给 `cargo test` 增加依赖与编译时间。
#![allow(dead_code)]

/// 64 位线性同余伪随机源（与 CJT188 既有测试同一算法）。
pub(crate) struct Lcg(u64);

impl Lcg {
    /// `seed` 为 0 时替换为常数，避免退化序列。
    pub(crate) fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    pub(crate) fn next_u8(&mut self) -> u8 {
        (self.next_u32() & 0xFF) as u8
    }

    /// `[0, n)`；`n == 0` 时返回 0。
    pub(crate) fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u32() as usize) % n
        }
    }

    pub(crate) fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_u8()).collect()
    }

    /// 不含 `0x68` 的随机字节。
    ///
    /// 用于「噪声 + 合法帧」的重同步测试：`0x68` 是 DLT645/CJT188 的起始字节，
    /// 噪声里出现它会让提取器按「数据不足」正常等待，属预期行为而非重同步失败。
    pub(crate) fn bytes_without_start_marker(&mut self, len: usize) -> Vec<u8> {
        (0..len)
            .map(|_| match self.next_u8() {
                0x68 => 0x69,
                b => b,
            })
            .collect()
    }
}
