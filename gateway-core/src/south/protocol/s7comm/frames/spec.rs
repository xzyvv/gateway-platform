//! 多 item Read/Write Var 的复用单元（决策表 #13）。
use super::*;

/// 单 item 变量规格（多 item Read/Write Var 帧的复用单元）
///
/// `count` 语义对齐 S7ANY：位类型为「位数」（固定 1），其它类型为「字节数」
/// （Byte=1, Word/Int=2, DWord/DInt/Real=4）。构造时由 `S7DataType` 推导。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ItemSpec {
    pub area: u8,
    pub db_number: u16,
    pub byte_offset: u32,
    pub bit_offset: u8,
    pub data_type: S7DataType,
}

impl ItemSpec {
    /// 按 data_type 计算 S7ANY.count 字段值
    pub(super) fn count(&self) -> u16 {
        if self.data_type == S7DataType::Bit {
            1
        } else {
            self.data_type.byte_size() as u16
        }
    }

    /// 3 字节大端位地址
    pub(super) fn bit_address(&self) -> [u8; 3] {
        let ba = self.byte_offset * 8 + self.bit_offset as u32;
        [
            ((ba >> 16) & 0xFF) as u8,
            ((ba >> 8) & 0xFF) as u8,
            (ba & 0xFF) as u8,
        ]
    }

    /// 单个 VariableSpecification（12 字节：头 3 + 余 9）
    pub(super) fn to_variable_spec(self) -> [u8; 12] {
        let mut v = [0u8; 12];
        v[0] = 0x12;
        v[1] = 0x0A;
        v[2] = 0x10; // Syntax ID: S7ANY
        v[3] = self.data_type.transport_size();
        v[4..6].copy_from_slice(&self.count().to_be_bytes());
        v[6..8].copy_from_slice(&self.db_number.to_be_bytes());
        v[8] = self.area;
        v[9..12].copy_from_slice(&self.bit_address());
        v
    }
}

/// 写方向单 item 的待写原始字节（已由调用方按 S7DataType 编码为字节序 BE）
#[derive(Debug, Clone, Copy)]
pub(crate) struct WriteItem<'a> {
    pub spec: ItemSpec,
    /// 原始字节（位类型 1 字节、字节 1 字节、字 2 字节……），不再编码
    pub value_le: &'a [u8],
}
