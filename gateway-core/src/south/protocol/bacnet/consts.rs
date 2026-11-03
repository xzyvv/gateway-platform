//! BACnet 协议常量：服务选择 / 对象类型 / 属性 / APDU 类型 / COV 时序 / 应用标签。

use std::time::Duration;

/// BACnet 服务选择
pub(crate) const SERVICE_READ_PROPERTY: u8 = 12;

/// BACnet 对象类型
pub(crate) const OBJECT_ANALOG_INPUT: u16 = 0;
pub(crate) const OBJECT_ANALOG_OUTPUT: u16 = 1;
pub(crate) const OBJECT_ANALOG_VALUE: u16 = 2;
pub(crate) const OBJECT_BINARY_INPUT: u16 = 3;
pub(crate) const OBJECT_BINARY_OUTPUT: u16 = 4;
pub(crate) const OBJECT_BINARY_VALUE: u16 = 5;

/// BACnet 属性 ID
pub(crate) const PROP_PRESENT_VALUE: u32 = 85;

/// BACnet 服务选择（决策表 #20）
pub(crate) const SERVICE_READ_PROPERTY_MULTIPLE: u8 = 14;
pub(crate) const SERVICE_WRITE_PROPERTY: u8 = 15;

/// S05-fix: 修复 frames.rs 编译错误——补齐被引用但未声明的 6 个 BACnet 标准常量。
/// 这些常量是 BACnet ASHRAE 135 协议规范定义的标准值，与现有 SERVICE_*/APDU_*
/// 命名风格一致；缺少它们会让 `who_is` / `i_am` / `subscribe_cov` /
/// `cov_notification` 等 APDU 构建/解析函数（frames.rs:632~）编译失败。
///
/// APDU PDU 类型扩展：补充 Unconfirmed-Request(0x10)
pub(crate) const APDU_UNCONFIRMED_REQUEST: u8 = 0x10;

/// BACnet 服务选择（Unconfirmed 域）——用于 Who-Is / I-Am / COVNotification
pub(crate) const SERVICE_WHO_IS: u8 = 8;
pub(crate) const SERVICE_I_AM: u8 = 0;
// COV 通路已由后台监听任务（`cov_listener`）接入生产路径，常量不再 gate。
pub(crate) const SERVICE_UNCONFIRMED_COV_NOTIFICATION: u8 = 2;

/// BACnet 服务选择（Confirmed 域）——SubscribeCOV
pub(crate) const SERVICE_SUBSCRIBE_COV: u8 = 27;

/// BACnet 对象类型（Device = 8）——I-Am 解析需要校验 objectIdentifier 高 10 位
pub(crate) const OBJECT_DEVICE: u16 = 8;
/// APDU PDU 类型（决策表 #20 写入响应判定）
pub(crate) const APDU_CONFIRMED_REQUEST: u8 = 0x00;
pub(crate) const APDU_SIMPLE_ACK: u8 = 0x20;
pub(crate) const APDU_COMPLEX_ACK: u8 = 0x30;
pub(crate) const APDU_ERROR: u8 = 0x50;
pub(crate) const APDU_REJECT: u8 = 0x60;
pub(crate) const APDU_ABORT: u8 = 0x70;

/// WriteProperty 写入优先级（8 = 手动操作员，楼宇 BAS 控制常用槽位）
pub(crate) const BACNET_WRITE_PRIORITY: u8 = 8;

/// COV 订阅生命周期（秒）：设备侧订阅在此之前失效，必须续订
pub(crate) const COV_LIFETIME_SECS: u32 = 300;
/// 续订周期：留出一次失败重试的余量（必须小于生命周期）
pub(crate) const COV_RENEW_INTERVAL: Duration = Duration::from_secs(240);
/// 订阅请求未获确认时的重试间隔，避免对无响应设备持续刷包
pub(crate) const COV_RETRY_INTERVAL: Duration = Duration::from_secs(10);
/// 监听任务调度周期：发现新登记点位、执行到期续订
pub(crate) const COV_TICK: Duration = Duration::from_millis(500);
/// 订阅者进程号（BACnet 只要求同一订阅方地址内唯一）
pub(crate) const COV_SUBSCRIBER_PID: u32 = 1;
/// COV 缓存有效期（毫秒）：与订阅生命周期保持一致
pub(crate) const COV_LIFETIME_MS: i64 = COV_LIFETIME_SECS as i64 * 1000;

/// BACnet 应用标签
pub(crate) const TAG_NULL: u8 = 0;
pub(crate) const TAG_BOOLEAN: u8 = 1;
pub(crate) const TAG_UNSIGNED: u8 = 2;
pub(crate) const TAG_SIGNED: u8 = 3;
pub(crate) const TAG_REAL: u8 = 4;
pub(crate) const TAG_DOUBLE: u8 = 5;
pub(crate) const TAG_ENUMERATED: u8 = 9;
