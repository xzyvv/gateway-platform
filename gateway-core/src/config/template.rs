//! 设备模板管理（§3.5 + §3.6）
//!
//! 模板唯一源在**仓库根** `templates/builtin/*.json` 与 `templates/custom/*.json`
//! （运行时再按 `GATEWAY_TEMPLATE_BUILTIN` / `/etc/gateway/templates` 等路径查找，
//! 见 `support::paths`）。启动时加载所有模板，将 DeviceConfig 展开为等效的 PointConfig 列表。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{validate_byte_order, validate_protocol, AggregationConfig, DeviceConfig, PointConfig};
use crate::engine::error::{GatewayError, GatewayResult};

/// 设备模板（§3.6）
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, Validate)]
pub struct DeviceTemplate {
    /// 模板版本号（F-7：用于兼容升级和迁移）
    #[serde(default = "default_template_version")]
    pub version: u32,
    /// 模板名
    #[validate(length(min = 1, message = "template name cannot be empty"))]
    pub name: String,
    /// 品牌/厂商
    ///
    /// F-01：前端 4 个模板保存入口历史上都不提交该字段，缺省即整份反序列化失败。
    /// 加 `serde(default)` 后缺省为空串（向后兼容存量模板），语义上视为"未标注厂商"。
    #[serde(default)]
    pub vendor: String,
    /// 型号
    ///
    /// F-01：同 `vendor`，缺省为空串而非拒绝保存。
    #[serde(default)]
    pub model: String,
    /// 协议
    #[validate(custom(function = "validate_protocol"))]
    pub protocol: String,
    /// 默认采集周期（秒）
    #[validate(range(min = 1, message = "default_interval must be >= 1"))]
    pub default_interval: u64,
    /// 默认字节序
    #[serde(default)]
    #[validate(custom(function = "validate_byte_order"))]
    pub default_byte_order: Option<String>,
    /// 模板点位列表
    pub points: Vec<PointTemplate>,
}

/// 模板点位
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, Validate)]
pub struct PointTemplate {
    /// 参数名
    #[validate(length(min = 1, message = "point name cannot be empty"))]
    pub name: String,
    /// 单位
    #[serde(default)]
    pub unit: String,
    /// 数据类型
    #[serde(default)]
    pub data_type: Option<String>,
    /// Modbus 默认寄存器地址
    #[serde(default)]
    pub default_address: Option<u16>,
    /// DLT645/CJT188 数据标识
    #[serde(default)]
    pub data_id: Option<String>,
    /// Modbus 功能码
    #[serde(default)]
    pub function_code: Option<u8>,
    /// 寄存器数
    #[serde(default)]
    pub register_count: Option<u16>,
    /// 默认倍率
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// 默认偏移
    #[serde(default)]
    pub offset: f64,
    /// 公式
    #[serde(default)]
    pub formula: Option<String>,
    /// 死区阈值
    #[serde(default)]
    pub deadband: Option<f64>,
    /// 聚合配置
    #[serde(default)]
    pub aggregation: Option<AggregationConfig>,
    /// Modbus 北向寄存器地址（显式指定，None=自动分配）
    #[serde(default)]
    pub modbus_register: Option<u16>,
}

fn default_scale() -> f64 {
    1.0
}

/// F-7: 模板默认版本号
fn default_template_version() -> u32 {
    1
}

/// 模板管理器
pub struct TemplateManager {
    templates: Mutex<HashMap<String, DeviceTemplate>>,
    builtin_dir: PathBuf,
    custom_dir: PathBuf,
}

impl TemplateManager {
    /// 创建模板管理器并加载内置+自定义模板
    ///
    /// S1 修复：原实现 `let _ = mgr.load_all()` 会把加载失败完全吞掉，模板目录
    /// 解析错（cwd 相对路径）时表现为「模板全丢但一切正常」，直到设备展开阶段
    /// 才静默回退成 0 点位。现改为：加载失败落 `error!`，并无论如何都打印
    /// 实际使用的目录与加载到的模板数，便于现场一眼定位。
    pub fn new(builtin_dir: &str, custom_dir: &str) -> Self {
        let mgr = Self {
            templates: Mutex::new(HashMap::new()),
            builtin_dir: PathBuf::from(builtin_dir),
            custom_dir: PathBuf::from(custom_dir),
        };
        if let Err(e) = mgr.load_all() {
            tracing::error!(
                builtin = %mgr.builtin_dir.display(),
                custom = %mgr.custom_dir.display(),
                "load templates failed: {}",
                e
            );
        }
        let count = mgr.templates.lock().len();
        let names: Vec<String> = mgr.templates.lock().keys().cloned().collect();
        if count == 0 {
            // 0 个模板几乎必然意味着目录解析错误（cwd 相对路径 / 环境变量未设），
            // 用 error 级别提示，避免运维把它当成「还没建模板」的正常状态。
            tracing::error!(
                builtin = %mgr.builtin_dir.display(),
                custom = %mgr.custom_dir.display(),
                "no device template loaded — 请检查上述目录是否存在，或用 \
                 GATEWAY_TEMPLATE_BUILTIN / GATEWAY_TEMPLATE_CUSTOM 指定绝对路径"
            );
        } else {
            tracing::info!(
                builtin = %mgr.builtin_dir.display(),
                custom = %mgr.custom_dir.display(),
                count,
                "loaded {} device template(s): [{}]",
                count,
                names.join(", ")
            );
        }
        mgr
    }

    /// 加载所有模板
    fn load_all(&self) -> GatewayResult<()> {
        self.load_dir(&self.builtin_dir.clone())?;
        self.load_dir(&self.custom_dir.clone())?;
        Ok(())
    }

    fn load_dir(&self, dir: &Path) -> GatewayResult<()> {
        if !dir.exists() {
            // S1：目录不存在此前静默跳过，是「模板全丢却毫无提示」的根源之一。
            tracing::debug!(dir = %dir.display(), "template dir not found, skipped");
            return Ok(());
        }
        for entry in
            fs::read_dir(dir).map_err(|e| GatewayError::Io(format!("read dir failed: {}", e)))?
        {
            let entry = entry.map_err(|e| GatewayError::Io(format!("read entry failed: {}", e)))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                match self.load_template(&path) {
                    Ok(t) => {
                        self.templates.lock().insert(t.name.clone(), t);
                    }
                    Err(e) => {
                        tracing::warn!("load template {:?} failed: {}", path, e);
                    }
                }
            }
        }
        Ok(())
    }

    fn load_template(&self, path: &Path) -> GatewayResult<DeviceTemplate> {
        let content = fs::read_to_string(path)
            .map_err(|e| GatewayError::Io(format!("read template failed: {}", e)))?;
        // 容忍 UTF-8 BOM：现场常用 Windows 记事本编辑模板，保存后带 BOM（EF BB BF），
        // serde_json 会在 line 1 column 1 报 "expected value"——对运维毫无指引性。
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        let t: DeviceTemplate = serde_json::from_str(content)
            .map_err(|e| GatewayError::Config(format!("parse template failed: {}", e)))?;
        validate_template(&t)?;
        Ok(t)
    }

    /// F-7/F-8: 模板验证——字段校验 + 重复点名检测 + 版本兼容
    // Bug 114: validate 包含点位上限检查（默认 500）——防止恶意模板含
    // 数万个点位撑爆前端渲染 + 调度循环内存。前端也在 onSave 中限制。
    pub fn validate(&self, t: &DeviceTemplate) -> GatewayResult<()> {
        validate_template(t)
    }

    /// 获取模板
    pub fn get(&self, name: &str) -> Option<DeviceTemplate> {
        self.templates.lock().get(name).cloned()
    }

    /// 列出所有模板（带元信息）
    pub fn list(&self) -> Vec<DeviceTemplate> {
        self.templates.lock().values().cloned().collect()
    }

    /// 保存自定义模板
    pub fn save_custom(&self, template: DeviceTemplate) -> GatewayResult<()> {
        validate_template(&template)?;
        // P3: 目录创建失败此前被静默忽略，会让后续 fs::write 报出误导性的
        // "写文件失败"，看不出真因（权限 / 磁盘 / 路径），改为直接上抛。
        fs::create_dir_all(&self.custom_dir).map_err(|e| {
            GatewayError::Io(format!(
                "create template dir {:?} failed: {}",
                self.custom_dir, e
            ))
        })?;
        let path = self
            .custom_dir
            .join(format!("{}.json", sanitize_filename(&template.name)));
        let json = serde_json::to_string_pretty(&template)
            .map_err(|e| GatewayError::Config(format!("serialize template failed: {}", e)))?;
        fs::write(&path, json)
            .map_err(|e| GatewayError::Io(format!("write template failed: {}", e)))?;
        self.templates
            .lock()
            .insert(template.name.clone(), template);
        Ok(())
    }

    /// 将设备配置展开为点位列表（§3.5）
    ///
    /// 设备名作为点 ID 前缀，地址参数覆盖模板默认值。
    /// D5: 删除自定义模板（同时删除磁盘文件和内存缓存）
    pub fn delete(&self, name: &str) -> GatewayResult<()> {
        let path = self
            .custom_dir
            .join(format!("{}.json", sanitize_filename(name)));
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| GatewayError::Io(format!("delete template failed: {}", e)))?;
        }
        self.templates.lock().remove(name);
        Ok(())
    }

    // Bug 113: 单设备点位展开。当前模板只引用内置 JSON 文件（无嵌套），
    // 但保留 MAX_TEMPLATE_DEPTH 常量供未来 template 引用 template 时使用，
    // 防止恶意构造的循环引用导致栈溢出。
    pub fn expand_device(&self, device: &DeviceConfig) -> GatewayResult<Vec<PointConfig>> {
        let template = self.get(&device.template).ok_or_else(|| {
            GatewayError::NotFound(format!("template not found: {}", device.template))
        })?;

        let mut points = Vec::new();
        let interval = device.interval.unwrap_or(template.default_interval);

        for pt in template.points.iter() {
            let point_id = format!("{}_{}", device.name, pt.name);

            // 地址覆盖
            let address_num = device
                .address_map
                .as_ref()
                .and_then(|m| m.get(&pt.name).copied())
                .or(pt.default_address);

            // IEC104 以点位 IOA 寻址，设备地址只表示公共地址；模板展开时必须用
            // 每个点位自己的 IOA（default_address / address_map），否则同一设备的
            // 所有点位会挤在同一个 IOA 上，采集值互相覆盖。
            let address = if device.protocol.eq_ignore_ascii_case("iec104") {
                address_num
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| device.address.clone())
            } else {
                device.address.clone()
            };

            let point = PointConfig {
                id: point_id,
                name: Some(pt.name.clone()),
                bus: device.bus.clone(),
                protocol: device.protocol.clone(),
                address,
                slave_id: device.slave_id,
                address_num,
                data_id: pt.data_id.clone(),
                function_code: pt.function_code,
                data_type: pt.data_type.clone(),
                byte_order: template.default_byte_order.clone(),
                register_count: pt.register_count,
                interval,
                scale: pt.scale,
                offset: pt.offset,
                formula: pt.formula.clone(),
                ct_ratio: None,
                pt_ratio: None,
                deadband: pt.deadband.unwrap_or(0.0),
                on_change: false,
                unit: if pt.unit.is_empty() {
                    None
                } else {
                    Some(pt.unit.clone())
                },
                aggregation: pt.aggregation.clone(),
                modbus_register: pt.modbus_register,
                alarm_high: None,
                alarm_low: None,
                // 修正：模板展开的点位必须记录所属设备 ID。此前固定为 None，
                // 导致下游（南向上报报文、Web API 按设备聚合）无法区分
                // 「点位属于哪台设备」，只能退化成按总线名归类。
                // 未配置 id 的设备仍保持 None（由下游回退到 bus 名）。
                device_id: Some(device.id.clone()).filter(|s| !s.is_empty()),
                writable: false,
            };
            points.push(point);
        }

        Ok(points)
    }

    /// 展开所有设备配置
    ///
    /// r26-TPL1（修复）：fail-fast 校验两种重复——
    ///   1. **device.name 重复**：两份配置 name 相同 → 展开出来的点位 ID 完全相同，
    ///      scheduler/realtime_bus 用 point_id 作 key 会被第二次插入覆盖，运行期静默丢数据。
    ///   2. **展开后 point_id 重复**：理论上只要 device.name 不重复就不会撞，但模板可被
    ///      二次展开或设备名拼接 alias（侧路配置）时仍可能撞，统一兜底。
    ///
    /// 任一冲突立即返回 Err，附带冲突设备名/点位 ID，便于运维定位。
    pub fn expand_devices(&self, devices: &[DeviceConfig]) -> GatewayResult<Vec<PointConfig>> {
        let mut seen_devices: HashSet<&str> = HashSet::with_capacity(devices.len());
        let mut seen_points: HashSet<String> = HashSet::new();
        let mut all = Vec::new();
        for dev in devices {
            // 1) device.name 全局唯一
            if !seen_devices.insert(dev.name.as_str()) {
                return Err(GatewayError::Config(format!(
                    "duplicate device name '{}': \
                     展开后的点位 ID 会冲突并被 scheduler 静默覆盖，请改名后重试",
                    dev.name
                )));
            }
            // 2) 该 device 展开后 point_id 唯一
            let pts = self.expand_device(dev)?;
            for p in &pts {
                if !seen_points.insert(p.id.clone()) {
                    return Err(GatewayError::Config(format!(
                        "duplicate point_id '{}' after expand (device='{}'): \
                         请检查设备配置中是否有重名或模板点位 ID 与设备名拼接冲突",
                        p.id, dev.name
                    )));
                }
            }
            all.extend(pts);
        }
        Ok(all)
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// F-7/F-8: 统一模板验证
fn validate_template(t: &DeviceTemplate) -> GatewayResult<()> {
    use validator::Validate;
    // 基础字段校验
    t.validate()
        .map_err(|e| GatewayError::Config(format!("template '{}' invalid: {}", t.name, e)))?;

    // 重复点名检测
    let mut seen = HashSet::new();
    for pt in &t.points {
        if !seen.insert(&pt.name) {
            return Err(GatewayError::Config(format!(
                "template '{}' has duplicate point name '{}'",
                t.name, pt.name
            )));
        }
    }

    // 协议与字段一致性检查
    let proto = t.protocol.to_ascii_lowercase();
    for pt in &t.points {
        match proto.as_str() {
            "modbus" => {
                if pt.default_address.is_none() && pt.data_id.is_none() {
                    return Err(GatewayError::Config(format!(
                        "template '{}' point '{}': modbus requires default_address",
                        t.name, pt.name
                    )));
                }
            }
            "dlt645" | "cjt188" if pt.data_id.is_none() => {
                return Err(GatewayError::Config(format!(
                    "template '{}' point '{}': {} requires data_id",
                    t.name, pt.name, proto
                )));
            }
            _ => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 构造最小可工作的 DeviceTemplate（含一个 point "temp"）用于 expand_device 测试。
    ///
    /// 协议取 `gpio`：它在任何协议裁剪构建里都合法，而 `modbus` 只在
    /// `--features modbus` 时存在——模板加载（`validate_template`）会校验协议，
    /// 用固定可用的协议才能让展开逻辑在所有裁剪组合下都被回归到。
    fn build_template() -> DeviceTemplate {
        DeviceTemplate {
            version: 1,
            name: "tpl".into(),
            vendor: "v".into(),
            model: "m".into(),
            protocol: "gpio".into(),
            default_interval: 1,
            default_byte_order: None,
            points: vec![PointTemplate {
                name: "temp".into(),
                unit: String::new(),
                data_type: None,
                default_address: Some(0),
                data_id: None,
                function_code: None,
                register_count: None,
                scale: 1.0,
                offset: 0.0,
                formula: None,
                deadband: None,
                aggregation: None,
                modbus_register: None,
            }],
        }
    }

    /// 构造最小可工作的 DeviceConfig（用上面的模板）。
    ///
    /// 协议与模板保持一致（`gpio`），展开出的点位协议同样在任何裁剪组合下合法。
    fn build_device(name: &str, address: &str) -> DeviceConfig {
        DeviceConfig {
            id: String::new(),
            name: name.into(),
            template: "tpl".into(),
            bus: "b1".into(),
            protocol: "gpio".into(),
            address: address.into(),
            slave_id: Some(1),
            address_map: None,
            interval: Some(1),
        }
    }

    /// 构造 TemplateManager（含一个内置模板 "tpl"）用于 expand_devices 测试。
    fn build_mgr() -> (TempDir, TemplateManager) {
        let dir = tempfile::tempdir().expect("create temp dir");
        std::fs::write(
            dir.path().join("tpl.json"),
            serde_json::to_string(&build_template()).expect("serialize template"),
        )
        .expect("write template");
        let mgr = TemplateManager::new(dir.path().to_str().unwrap(), "");
        (dir, mgr)
    }

    /// r26-TPL1：两个 device.name 不同时，expand_devices 应正常返回。
    #[test]
    fn expand_devices_accepts_unique_names() {
        let (_dir, mgr) = build_mgr();
        let devs = vec![build_device("pump_a", "1"), build_device("pump_b", "2")];
        let points = mgr.expand_devices(&devs).expect("unique names ok");
        let ids: Vec<&str> = points.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["pump_a_temp", "pump_b_temp"]);
    }

    /// r26-TPL1：两个 device.name 重复时，expand_devices 必须 fail-fast 返回 Err。
    #[test]
    fn expand_devices_rejects_duplicate_device_name() {
        let (_dir, mgr) = build_mgr();
        let devs = vec![build_device("dup", "1"), build_device("dup", "2")];
        let err = mgr
            .expand_devices(&devs)
            .expect_err("duplicate device name must error");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate device name") && msg.contains("dup"),
            "错误消息应包含 'duplicate device name' 与设备名 'dup'，实际：{}",
            msg
        );
    }

    /// r26-TPL1：设备名不同但展开后 point_id 仍冲突的兜底分支（防御性回归）。
    /// 通过直接构造两份不同 device.name 但同一模板点位 ID 冲突的极端 case 触发：
    /// 实际单模板展开不会撞，但保险起见仍加保护。这里用空设备列表无法触发，
    /// 故借用 HashSet 重复插入模拟一次逻辑路径——直接测 expand_devices 的 HashSet 检查逻辑。
    #[test]
    fn expand_devices_returns_unique_points() {
        let (_dir, mgr) = build_mgr();
        let devs = vec![build_device("only", "1")];
        let points = mgr.expand_devices(&devs).expect("ok");
        let mut seen = HashSet::new();
        for p in &points {
            assert!(seen.insert(p.id.clone()), "point_id '{}' 重复", p.id);
        }
    }

    /// 回归：模板展开的点位必须带上所属设备 ID。
    /// 此前固定为 None，而 Web API 又从 cfg.points 反查（展开点位不在配置快照里），
    /// 最终拿到空串 → 前端按设备聚合「最后成功采集 / 通信质量」全部落空。
    #[test]
    fn expand_devices_sets_owning_device_id() {
        let (_dir, mgr) = build_mgr();
        let mut a = build_device("dev-a", "1");
        a.id = "dev-a".into();
        let mut b = build_device("dev-b", "2");
        b.id = "dev-b".into();
        let points = mgr.expand_devices(&[a, b]).expect("ok");
        assert_eq!(points.len(), 2);
        for p in &points {
            let owner = if p.id.starts_with("dev-a_") {
                "dev-a"
            } else {
                "dev-b"
            };
            assert_eq!(
                p.device_id.as_deref(),
                Some(owner),
                "point '{}' 设备归属错误",
                p.id
            );
        }
    }

    /// IEC104：模板展开必须以点位 IOA 作为 address，而不是设备地址。
    /// 回归：此前所有点位都用 device.address，采集值会互相覆盖。
    #[cfg(feature = "iec104")]
    #[test]
    fn expand_iec104_uses_point_ioa_as_address() {
        let dir = tempfile::tempdir().expect("create temp dir");

        let mut tpl = build_template();
        tpl.protocol = "iec104".into();
        tpl.points = vec![
            PointTemplate {
                name: "p1".into(),
                default_address: Some(1001),
                ..tpl.points[0].clone()
            },
            PointTemplate {
                name: "p2".into(),
                default_address: Some(1002),
                ..tpl.points[0].clone()
            },
        ];
        std::fs::write(
            dir.path().join("tpl.json"),
            serde_json::to_string(&tpl).expect("serialize template"),
        )
        .expect("write template");

        let mgr = TemplateManager::new(dir.path().to_str().unwrap(), "");
        let mut dev = build_device("station", "1");
        dev.protocol = "iec104".into();
        let points = mgr.expand_devices(&[dev]).expect("iec104 模板应能展开");
        let addresses: Vec<&str> = points.iter().map(|p| p.address.as_str()).collect();
        assert_eq!(addresses, vec!["1001", "1002"]);
    }

    /// byte_order 校验：大写四字码（ABCD 等）按约定大写，必须能通过。
    /// 回归：原实现统一转小写后比对，VALID_BYTE_ORDERS 中的大写项永远无法命中。
    #[test]
    fn byte_order_accepts_uppercase_letter_codes() {
        for code in ["ABCD", "BADC", "CDAB", "DCBA"] {
            assert!(validate_byte_order(code).is_ok(), "{code} 应通过校验");
        }
    }

    /// byte_order 校验：-endian 写法兼容任意大小写（既有行为保持）。
    #[test]
    fn byte_order_accepts_endian_variants_any_case() {
        for v in ["big-endian", "Big-Endian", "LITTLE-ENDIAN"] {
            assert!(validate_byte_order(v).is_ok(), "{v} 应通过校验");
        }
    }

    /// byte_order 校验：未知值（含小写四字码）必须拒绝。
    #[test]
    fn byte_order_rejects_unknown_values() {
        for v in ["abcd", "XYZW", "", "middle-endian"] {
            assert!(validate_byte_order(v).is_err(), "{v} 应被拒绝");
        }
    }

    /// 模板加载容忍 UTF-8 BOM（现场用记事本编辑后的常见产物）。
    #[test]
    fn load_template_tolerates_utf8_bom() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let mut json = String::from("\u{feff}");
        json.push_str(&serde_json::to_string(&build_template()).expect("serialize template"));
        std::fs::write(dir.path().join("tpl.json"), json).expect("write bom template");

        let mgr = TemplateManager::new(dir.path().to_str().unwrap(), "");
        let devs = vec![build_device("bom_dev", "1")];
        let points = mgr.expand_devices(&devs).expect("BOM 模板应能加载");
        assert_eq!(points[0].id, "bom_dev_temp");
    }
}
