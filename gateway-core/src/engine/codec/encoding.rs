//! Base64 编解码（标准字母表，带 = 填充）
//!
//! 拆出 mod.rs 内部实现，供 secret.rs / password.rs 复用。

const B64_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// r23-ENC1 修复：Base64 解码输入硬上限（16 KiB 过滤后字节数）。
///
/// 旧实现无长度校验——攻击者传入 `"A" * 1_000_000` 会让
/// `Vec::with_capacity(bytes.len() * 3 / 4)` 一次性分配 ~750 KiB，
/// 多次调用可放大内存压力（虽然调用方目前都是内部模块，但属于防御性下限）。
///
/// 取值依据：
/// - secret.rs 中加密的配置/私钥长度通常 < 1 KiB，16 KiB 上限远超合法输入。
/// - 留出足够余量给将来的证书 / 大型 payload 场景。
const B64_MAX_INPUT_BYTES: usize = 16 * 1024;

pub(crate) fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = |i: usize| -> u8 { *chunk.get(i).unwrap_or(&0) };
        let n = ((b(0) as u32) << 16) | ((b(1) as u32) << 8) | (b(2) as u32);
        out.push(B64_ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_ALPHABET[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_ALPHABET[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub(crate) fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|&c| c != b'\n' && c != b'\r' && c != b' ')
        .collect();
    // r23-ENC1：长度上限防御。过滤后字节数超过硬上限视为非法输入，
    // 返回 None（调用方按"解码失败"处理，如 secret.rs 触发重试/告警）。
    if bytes.len() > B64_MAX_INPUT_BYTES {
        return None;
    }
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'=' {
            break;
        }
        let n0 = val(bytes[i])?;
        let n1 = if bytes[i + 1] == b'=' {
            break;
        } else {
            val(bytes[i + 1])?
        };
        out.push(((n0 << 2) | (n1 >> 4)) as u8);
        if i + 2 < bytes.len() && bytes[i + 2] != b'=' {
            let n2 = val(bytes[i + 2])?;
            out.push((((n1 & 0xF) << 4) | (n2 >> 2)) as u8);
            if i + 3 < bytes.len() && bytes[i + 3] != b'=' {
                let n3 = val(bytes[i + 3])?;
                out.push((((n2 & 0x3) << 6) | n3) as u8);
            }
        }
        i += 4;
    }
    Some(out)
}

#[cfg(test)]
mod tests;
