//! S6：配置向后兼容回归（crate 内单测）
//!
//! 锁定两条不变量，防止未来改动悄悄破坏现场升级路径：
//!
//! 1. **旧配置样本仍可加载**：历史/现场可能写入的配置形态，必须能通过
//!    结构校验 → serde 反序列化 → 版本迁移 → 跨字段业务校验 全链路。
//! 2. **Schema 字段契约不收缩**：相对基线，配置字段只允许新增，
//!    不允许删除 / 改名 / 改类型（含 required 集变化）。
//!
//! 为什么放在 crate 内（而非 `tests/`）：
//! - 需要调用 `pub(crate)` 的 `validate_all()`；
//! - 落在 `cargo test --lib` 内，天然被 CI 现有门禁覆盖，无需改 ci.yml。

use super::{schema, GatewayConfig, CURRENT_SCHEMA_VERSION};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// 旧配置样本：名称 → 内容（`include_str!` 编译期内嵌，无运行时路径依赖）。
///
/// 每个样本代表一种"曾经可能发布 / 写入现场"的配置形态：
/// - `v0_minimal`：无 `schema_version` 的历史最小配置（走 v0→v1 迁移）
/// - `v1_mqtt_ntp`：带 MQTT + NTP 的常见配置
/// - `bus_reconnect_backoff`：显式 `reconnect_backoff` + 默认退避的混合总线 + 一个完整点位
/// - `cellular_legacy`：**早于** 6 个阶段超时字段存在的 4G 配置（验证默认值补齐）
/// - `unknown_legacy_fields`：带未知字段（模拟跨版本写入）——必须被容忍
const FIXTURES: &[(&str, &str)] = &[
    (
        "v0_minimal",
        include_str!("fixtures/config/v0_minimal.json"),
    ),
    (
        "v1_mqtt_ntp",
        include_str!("fixtures/config/v1_mqtt_ntp.json"),
    ),
    (
        "bus_reconnect_backoff",
        include_str!("fixtures/config/bus_reconnect_backoff.json"),
    ),
    (
        "cellular_legacy",
        include_str!("fixtures/config/cellular_legacy.json"),
    ),
    (
        "unknown_legacy_fields",
        include_str!("fixtures/config/unknown_legacy_fields.json"),
    ),
];

/// Schema 契约基线的仓库内相对路径。
const BASELINE_REL: &str = "src/config/fixtures/config/schema_baseline.txt";

fn baseline_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BASELINE_REL)
}

// ============================================================
// A. 旧配置样本回归
// ============================================================

/// 样本引用的协议是否都在当前构建中启用。
///
/// 协议裁剪构建（`--no-default-features --features ...`）下，未编译的协议其历史配置
/// 本就装不进来——那不是向后兼容问题，而是该协议在这个构建里不存在。跳过这类样本，
/// 其余样本仍照常回归。
#[cfg(test)]
fn protocols_available(cfg: &GatewayConfig) -> bool {
    use crate::domain::VALID_PROTOCOLS;
    cfg.buses
        .iter()
        .map(|b| b.protocol.as_str())
        .chain(cfg.points.iter().map(|p| p.protocol.as_str()))
        .chain(
            cfg.devices
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|d| d.protocol.as_str()),
        )
        .all(|p| VALID_PROTOCOLS.contains(&p))
}

/// 每个样本走完整加载管线，任一环节失败即视为"升级后配置起不来"。
#[test]
fn legacy_config_samples_still_load_migrate_and_validate() {
    for (name, raw) in FIXTURES {
        // 1) 原始 JSON 合法
        let value: serde_json::Value =
            serde_json::from_str(raw).unwrap_or_else(|e| panic!("样本 {name} 非合法 JSON: {e}"));

        // 2) Schema 结构层校验（应容忍未知旧字段）
        if let Err(errs) = schema::validate_value(&value) {
            panic!("样本 {name} 未通过结构校验: {errs:?}");
        }

        // 3) serde 反序列化
        let mut cfg: GatewayConfig = serde_json::from_value(value)
            .unwrap_or_else(|e| panic!("样本 {name} 反序列化失败: {e}"));

        // 3.5) 协议裁剪构建：样本引用的协议未编译进来时跳过（见 protocols_available）
        if !protocols_available(&cfg) {
            continue;
        }

        // 4) 版本迁移到位
        cfg.migrate()
            .unwrap_or_else(|e| panic!("样本 {name} 迁移失败: {e}"));
        assert_eq!(
            cfg.schema_version, CURRENT_SCHEMA_VERSION,
            "样本 {name} 迁移后 schema_version 应为当前版本"
        );

        // 5) 跨字段业务校验
        if let Err(errs) = cfg.validate_all() {
            panic!("样本 {name} 未通过跨字段校验: {errs:?}");
        }
    }
}

// ============================================================
// B. Schema 字段契约不收缩
// ============================================================

/// 类型签名归一：把 schemars 产物折算为稳定、可比对的字符串。
///
/// - `$ref` → 定义名（如 `MqttConfig`），既稳定又能表达"指向哪个结构"；
/// - `anyOf`/`oneOf` → 各分支签名去重排序、并剔除 `null`（schemars 用其表达 `Option<T>`）；
/// - `type` 字符串/数组 → 原样（数组排序后 join）；
/// - `enum` → `enum`；兜底 → `any`。
fn type_sig(node: &serde_json::Value) -> String {
    if let Some(r) = node.get("$ref").and_then(|v| v.as_str()) {
        return r.rsplit('/').next().unwrap_or(r).to_string();
    }
    if let Some(branches) = node
        .get("anyOf")
        .or_else(|| node.get("oneOf"))
        .and_then(|v| v.as_array())
    {
        let mut parts: Vec<String> = branches.iter().map(type_sig).collect();
        parts.retain(|s| s != "null");
        parts.sort();
        parts.dedup();
        if parts.is_empty() {
            return "null".to_string();
        }
        return parts.join("|");
    }
    if let Some(arr) = node.get("type").and_then(|v| v.as_array()) {
        let mut parts: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        parts.sort();
        return parts.join("|");
    }
    if let Some(s) = node.get("type").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if node.get("enum").is_some() {
        return "enum".to_string();
    }
    "any".to_string()
}

/// 收集单个定义的字段契约行：`Def.field = 类型` 与 `Def#required field`。
fn collect_def(def_name: &str, def: &serde_json::Value, out: &mut BTreeSet<String>) {
    if let Some(props) = def.get("properties").and_then(|p| p.as_object()) {
        for (field, fsch) in props {
            out.insert(format!("{def_name}.{field} = {}", type_sig(fsch)));
        }
    }
    if let Some(req) = def.get("required").and_then(|r| r.as_array()) {
        for r in req.iter().filter_map(|v| v.as_str()) {
            out.insert(format!("{def_name}#required {r}"));
        }
    }
}

/// 全量字段契约：根对象 + 所有 `$defs` 定义。
///
/// schemars 会把每个嵌套结构体放进 `$defs`，因此"逐定义列字段"即可覆盖
/// 全部配置类型的字段名与类型，无需深层递归——删除/改名/改类型都会在此显形。
fn field_contract_set(root: &serde_json::Value) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    collect_def("GatewayConfig", root, &mut out);
    if let Some(defs) = root.get("$defs").and_then(|d| d.as_object()) {
        for (name, def) in defs {
            collect_def(name, def, &mut out);
        }
    }
    out
}

/// 基线必须是当前契约的**子集**：新增放行，缺失（删/改名/改类型/required 变化）即失败。
#[test]
fn schema_field_contract_not_shrunk() {
    let baseline_raw = std::fs::read_to_string(baseline_path()).unwrap_or_else(|e| {
        panic!(
            "缺少 Schema 契约基线 {}: {e}\n请先运行: cargo test -p gateway-core --lib -- --ignored regen_schema_baseline",
            BASELINE_REL
        )
    });
    let baseline: BTreeSet<&str> = baseline_raw
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    let current = field_contract_set(schema::schema());

    let mut missing: Vec<&str> = baseline
        .iter()
        .copied()
        .filter(|l| !current.contains(*l))
        .collect();
    missing.sort();

    assert!(
        missing.is_empty(),
        "配置 Schema 契约发生收缩——字段被删除/改名/改类型或 required 集变化，\
破坏了向后兼容（旧配置可能无法加载）。缺失 {} 行:\n{}\n\n\
若确为有意的破坏性变更，须人工评审并同步刷新基线: \
cargo test -p gateway-core --lib -- --ignored regen_schema_baseline",
        missing.len(),
        missing.join("\n")
    );
}

/// 手动刷新基线：仅在**合法的**配置变更（新增字段等）后运行，
/// 并把更新后的 `schema_baseline.txt` 随提交一并带上（可审计）。
#[test]
#[ignore = "仅在配置发生合法变更后手动运行以刷新基线"]
fn regen_schema_baseline() {
    let lines = field_contract_set(schema::schema());
    let mut body = lines.into_iter().collect::<Vec<String>>().join("\n");
    body.push('\n');

    let path = baseline_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("创建基线目录失败: {e}"));
    }
    std::fs::write(&path, body).unwrap_or_else(|e| panic!("写入基线失败: {e}"));
    eprintln!("已写入 Schema 契约基线: {}", path.display());
}
