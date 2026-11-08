//! 死信 payload 预览的渲染规则。
//!
//! 从 `outbox.rs` 拆出（C24 行数预算），行为不变。

/// 预览的最大字节数（与列表接口的返回体积约定一致）。
pub(super) const PREVIEW_BYTES: usize = 512;
/// 二进制预览最多展示的字节数：hex 为 2 字符/字节，128 字节 = 256 字符，
/// 足够看出魔数与版本头，又不会把表格撑爆。
const PREVIEW_BINARY_BYTES: usize = 128;

/// 生成死信 payload 预览。
///
/// D5：`String::from_utf8_lossy` 会把二进制 payload（CBOR / gzip 帧 / 私有协议）
/// 里的每个非法字节换成 `�`，现场看到的是一串乱码问号——既判断不出内容，
/// 也看不出数据类型。因此这里区分两种情况：
/// - 合法 UTF-8：原样输出；
/// - 非法 UTF-8：改为 hex 并加 `0x` 前缀，至少能认出魔数（`0x1f8b` = gzip）
///   与真实长度。
///
/// 截断可能正好落在多字节字符中间。此时 `error_len().is_none()` 且错误位置在
/// 末尾，属于"被边界截断"而非"二进制数据"，输出前面的可读前缀，不整体降级成 hex。
pub(super) fn payload_preview(payload: &[u8]) -> String {
    let head = &payload[..payload.len().min(PREVIEW_BYTES)];
    match std::str::from_utf8(head) {
        Ok(s) => s.to_string(),
        Err(e) if e.error_len().is_none() && e.valid_up_to() + 4 > head.len() => {
            String::from_utf8_lossy(&head[..e.valid_up_to()]).to_string()
        }
        Err(_) => {
            use std::fmt::Write as _;
            let bin = &head[..head.len().min(PREVIEW_BINARY_BYTES)];
            let mut s = String::with_capacity(2 + bin.len() * 2 + 24);
            s.push_str("0x");
            for b in bin {
                let _ = write!(s, "{b:02x}");
            }
            if payload.len() > bin.len() {
                let _ = write!(s, " ({} bytes, hex)", payload.len());
            }
            s
        }
    }
}
