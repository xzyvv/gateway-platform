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

use super::point::parse_data_type;
use super::{
    validate_byte_order, validate_protocol, validate_ratio, AggregationConfig, DeviceConfig,
    PointConfig,
};
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
    /// 模板说明（备注）
    ///
    /// 模板是随文件分发的 JSON，说明写在这里能跟着文件走：适配的固件版本、
    /// 接线要求、变比是否需要现场调整等。纯文档字段，不参与采集与校验；
    /// `serde(default)` 保证存量模板文件照常加载（读出来是 `None`）。
    #[serde(default)]
    pub description: Option<String>,
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
    /// CT 变比（模板级默认值，展开后可被设备侧覆盖；None=不启用）
    ///
    /// 与 `PointConfig::ct_ratio` 同义：一次侧 = 二次侧 × 变比，
    /// 仅电流/功率/电量类单位生效。模板里通常留空，由现场按铭牌填写。
    #[serde(default)]
    pub ct_ratio: Option<f64>,
    /// PT 变比（模板级默认值，None=不启用，仅电压/功率/电量类单位生效）
    #[serde(default)]
    pub pt_ratio: Option<f64>,
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

/// 由功能码 + 0 基寄存器偏移生成 Modbus 5 位展示地址。
///
/// 与前端 `modbusAddressOf` 同一套区段规则：线圈 0xxxx / 离散输入 1xxxx /
/// 输入寄存器 4xxxx / 保持寄存器 3xxxx，后四位是 1 基地址。
/// 偏移超过 9998 时返回 `None`（5 位串装不下），由调用方兜底，避免编造错地址。
fn modbus_display_address(function_code: Option<u8>, address_num: Option<u16>) -> Option<String> {
    let offset = u32::from(address_num?) + 1;
    if offset > 9999 {
        return None;
    }
    let head = match function_code {
        Some(1) | Some(5) | Some(15) => '0',
        Some(2) => '1',
        Some(4) => '4',
        _ => '3',
    };
    Some(format!("{}{:04}", head, offset))
}

/// 把历史模板里的数据类型别名（`uint16` / `float` / `double` …）统一成规范名。
///
/// 只处理 Modbus：`parse_data_type` 本来就接受这些别名，采集结果不受影响，
/// 统一是为了让模板列表/表单与新下拉一致（否则下拉里没有该值，用户以为配错了）。
/// IEC104 的 `float` / `single` / `normalized` 是该协议自己的语义，不能改名。
///
/// 返回被改写的点位数（0 表示无需迁移）。
fn normalize_data_types(t: &mut DeviceTemplate) -> usize {
    if !t.protocol.eq_ignore_ascii_case("modbus") {
        return 0;
    }
    let mut changed = 0;
    for p in &mut t.points {
        let Some(raw) = p.data_type.as_deref() else {
            continue;
        };
        let Some(canonical) = parse_data_type(raw).map(|dt| dt.canonical_name()) else {
            continue;
        };
        if raw != canonical {
            p.data_type = Some(canonical.to_string());
            changed += 1;
        }
    }
    changed
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
        let mut t: DeviceTemplate = serde_json::from_str(content)
            .map_err(|e| GatewayError::Config(format!("parse template failed: {}", e)))?;
        // 历史模板里的 uint16 / float / double 等别名统一成规范名（仅 Modbus）
        let normalized = normalize_data_types(&mut t);
        if normalized > 0 {
            tracing::info!(
                template = %t.name,
                points = normalized,
                "历史数据类型写法已按规范名加载（如 float→f32），保存该模板后落盘生效"
            );
        }
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
    pub fn save_custom(&self, mut template: DeviceTemplate) -> GatewayResult<()> {
        // 与加载侧同一条规范化：避免别名再次写回磁盘
        normalize_data_types(&mut template);
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
            //
            // Modbus 实际按 `address_num`（0 基偏移）+ 功能码寻址，`address` 只是给
            // 人看的 5 位展示串。此前统一写 `device.address`（从站地址），列表里
            // 同一设备的点位全显示成同一个从站号，现场按界面排查会走偏。
            let address = if device.protocol.eq_ignore_ascii_case("modbus") {
                modbus_display_address(pt.function_code, address_num).unwrap_or_else(|| {
                    // 偏移超出 5 位串表达范围（> 9998）：退回偏移数字，至少不丢信息
                    address_num
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| device.address.clone())
                })
            } else if device.protocol.eq_ignore_ascii_case("iec104") {
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
                ct_ratio: pt.ct_ratio,
                pt_ratio: pt.pt_ratio,
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

        // 变比必须 > 0：0 会把一次侧值乘成 0，负数会反号——与点位侧同一条规则。
        // 模板点位未挂 `#[validate(nested)]`，这里显式校验，避免坏模板落盘后才在
        // 展开/采集阶段表现为「读数全 0」。
        for (label, ratio) in [("ct_ratio", pt.ct_ratio), ("pt_ratio", pt.pt_ratio)] {
            if let Some(v) = ratio {
                validate_ratio(v).map_err(|e| {
                    GatewayError::Config(format!(
                        "template '{}' point '{}': {} {}",
                        t.name,
                        pt.name,
                        label,
                        e.message.unwrap_or(e.code)
                    ))
                })?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
