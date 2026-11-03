//! 配置 Schema 版本迁移
//!
//! 按版本号逐步迁移（v0 → v1 → v2 → ...），每次迁移只做一步，
//! 确保升级路径清晰可追溯。

use super::{GatewayConfig, CURRENT_SCHEMA_VERSION};

impl GatewayConfig {
    /// 将配置从旧版本迁移到当前 Schema 版本。
    ///
    /// Bug 177: 返回 `Result`——原实现 `while schema_version < CURRENT` 对「版本号高于
    /// 当前支持版本」的配置直接跳出循环并静默接受，导致更新版本网关写出的不兼容配置
    /// 被静默加载。现在 `> CURRENT` 时返回 Err，由调用方阻断启动。
    pub fn migrate(&mut self) -> Result<(), String> {
        if self.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(format!(
                "配置 schema_version={} 高于当前支持的 {}：可能由更新版本网关写入，拒绝加载以避免不兼容配置被静默接受",
                self.schema_version, CURRENT_SCHEMA_VERSION
            ));
        }
        while self.schema_version < CURRENT_SCHEMA_VERSION {
            let from = self.schema_version;
            match from {
                0 => self.migrate_v0_to_v1(),
                _ => {
                    return Err(format!(
                        "未知的配置 schema 版本 {}，无法迁移到 {}",
                        from, CURRENT_SCHEMA_VERSION
                    ));
                }
            }
            self.schema_version = from + 1;
            tracing::info!("配置 schema 迁移: v{} -> v{}", from, from + 1);
        }
        Ok(())
    }

    /// v0 → v1 迁移
    ///
    /// v0 是无 schema_version 字段的历史配置。
    /// 当前无结构性变更，仅设置版本号（为未来变更预留扩展点）。
    fn migrate_v0_to_v1(&mut self) {
        // 预留：未来新增字段时在此设置默认值
        // 例如：self.mqtt.some_new_field = Some(default_value());
    }
}
