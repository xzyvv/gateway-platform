//! 协议层公共工具：编解码、工具函数、跨协议帧提取
//!
//! 注：共享领域模型 DataPoint 已上提至 `crate::engine::model`（原 model.rs
//! 位于此处导致 north 反向依赖 south）。

// IEC 60870-5 ASDU 共享层：iec104（TCP）与 iec101（串行）复用同一套 ASDU 编解码，
// 两者都关闭时不参与编译，避免无用代码与死代码告警。
#[cfg(any(feature = "iec104", feature = "iec101"))]
pub(crate) mod asdu;
pub mod decode;
// DLT645 / CJT188 共享帧层：两者都关闭时不参与编译——这三个函数仅被
// dlt645::frames 与 cjt188::frames 调用，单协议或零协议构建下会成为死代码，
// 在 `-D dead_code` 下直接报错（F1 协议裁剪矩阵）。
pub mod codec;
#[cfg(any(feature = "dlt645", feature = "cjt188"))]
pub mod frame_extract;
