//! 数据质量规则安装端口。
//!
//! 数据质量规则的执行实现位于 `crate::pipeline::quality_rules`，但 Web 写接口
//! （`gateway-api` 的 `web/api`）被分层规则禁止直接引用 `gateway_core::pipeline`。
//!
//! 与 `engine::ports` 的做法一致：**由中立层定义契约，由依赖方提供实现**。
//! 装配点创建 [`crate::pipeline::quality_rules::QualityRuleRuntime`] 实例，分别交给
//! 处理管道（执行）与 AppState（作为本 trait 对象供写接口重装规则），
//! 二者共享同一实例，规则变更即时生效。

/// 从规则存储读取并安装已启用规则的端口。
///
/// 由 `pipeline::QualityRuleRuntime` 实现。
pub trait QualityRuleInstaller: Send + Sync {
    /// 读取存储中已启用的规则并装到运行时，返回安装条数。
    ///
    /// 读取失败返回错误文本，由调用方转成 HTTP 5xx——避免「保存成功但规则未生效」
    /// 的假成功（见 DD-41）。
    fn install_from_store(&self, store: &crate::config::QualityRuleStore) -> Result<usize, String>;
}
