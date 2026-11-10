//! 极简 5 段 cron 表达式求值（分 时 日 月 周）
//!
//! 计划任务调度只需要「给定 cron 表达式与起点时刻，求下次触发时间」。
//! 不引入外部 cron crate（workspace 无此依赖），实现 Vixie cron 子集：
//! - 每段支持：`*`（任意）、`*/n`（步进）、`a`（单值）、`a-b`（区间）、`a-b/n`（区间步进）、
//!   逗号列表（如 `1,15,30`）
//! - 星期段：0 与 7 均表示周日
//! - 日/周语义遵循 Vixie 惯例：两者都受限（非 `*`）时取**并集**（OR），任一受限取交集
//!
//! 无法解析的表达式返回 None，由上层拒绝创建任务（fail-fast）。
//!
//! # 时区
//!
//! cron 的 5 个字段是**墙钟字段**（「每天凌晨 2 点」= 设备当地 02:00），
//! 因此求值必须知道设备本地时区。设备本地时区按以下优先级解析：
//!
//! 1. `GATEWAY_TZ` 环境变量：**显式固定 UTC 偏移**（`+08:00` / `UTC+8` / `Z`）。
//!    优先级最高，且不依赖设备是否安装 tzdata——工业现场常用精简镜像，
//!    `/usr/share/zoneinfo` 缺失时 OS 时区会静默退化成 UTC。
//! 2. 操作系统本地时区（`TZ` 环境变量或 `/etc/localtime`），含夏令时。
//! 3. 两者都取不到时回退 UTC，并打告警日志——时区错一次就是「差 8 小时」，
//!    属于必须让运维在日志里看见的问题，不允许静默。
//!
//! 生效的时区会在启动日志里打印（见 `tz_description`），运维可据此核对
//! 「每天 02:00」到底是哪儿的 02:00。

/// 解析结果：5 个字段各自的取值位图（bit i 置位表示值 i 命中）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CronSchedule {
    minutes: u64,    // 0..=59
    hours: u32,      // 0..=23
    month_days: u32, // 1..=31
    months: u16,     // 1..=12
    week_days: u8,   // 0..=6（0=周日）
    /// dom 或 dow 为 `*`（未受限）
    dom_any: bool,
    dow_any: bool,
}

/// 解析 cron 表达式；非法表达式返回 None。
pub fn parse(expr: &str) -> Option<CronSchedule> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return None;
    }
    let (minutes, _) = parse_field(fields[0], 0, 59)?;
    let (hours, _) = parse_field(fields[1], 0, 23)?;
    let (month_days, dom_any) = parse_field(fields[2], 1, 31)?;
    let (months, _) = parse_field(fields[3], 1, 12)?;
    let (mut week_days, dow_any) = parse_field(fields[4], 0, 7)?;
    // 7 = 周日 → 归一到 0
    if week_days & (1 << 7) != 0 {
        week_days |= 1;
        week_days &= !(1 << 7);
    }
    Some(CronSchedule {
        minutes,
        hours: hours as u32,
        month_days: month_days as u32,
        months: months as u16,
        week_days: week_days as u8,
        dom_any,
        dow_any,
    })
}

/// 解析单个字段为位图。`max` 为字段允许的最大值（含）。
/// 返回 (位图, 是否未受限即 `*`)。位图用 u64：最大 60 种取值（分钟段）足够装下。
fn parse_field(field: &str, min: u32, max: u32) -> Option<(u64, bool)> {
    let mut bits: u64 = 0;
    let mut any = false;
    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        // 拆分 base/step：`*/5`、`1-10/2`、`5`
        let (base, step) = match part.split_once('/') {
            Some((b, s)) => (b, s.parse::<u32>().ok().filter(|n| *n > 0)?),
            None => (part, 1),
        };
        let (lo, hi) = match base {
            "*" => {
                any = true;
                (min, max)
            }
            _ => {
                if let Some((a, b)) = base.split_once('-') {
                    (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)
                } else {
                    let v = base.parse::<u32>().ok()?;
                    // 单值带步进 `a/n`：语义为 a..max 每 n（Vixie 惯例）
                    if part.contains('/') {
                        (v, max)
                    } else {
                        (v, v)
                    }
                }
            }
        };
        if lo < min || hi > max || lo > hi {
            return None;
        }
        let mut v = lo;
        while v <= hi {
            bits |= 1u64 << v;
            v += step;
        }
    }
    Some((bits, any))
}

impl CronSchedule {
    /// 给定时刻（UNIX 秒）是否命中——按**设备本地时区**解释 cron 字段
    pub fn matches(&self, ts_secs: i64) -> bool {
        self.matches_in(ts_secs, device_offset_secs(ts_secs))
    }

    /// 按指定 UTC 偏移（秒，东为正）判断是否命中
    ///
    /// 拆出显式偏移版本：测试可固定时区，将来也可按设备/租户时区执行。
    pub fn matches_in(&self, ts_secs: i64, offset_secs: i32) -> bool {
        // 把时刻平移到目标时区的墙钟（与 with_timezone 等价，但无需构造 FixedOffset）
        let wall = ts_secs.saturating_add(offset_secs as i64);
        let dt = chrono::DateTime::from_timestamp(wall, 0).unwrap_or_default();
        use chrono::Datelike;
        use chrono::Timelike;
        let minute = dt.minute() as u64;
        let hour = dt.hour();
        let dom = dt.day();
        let month = dt.month();
        let dow = dt.weekday().num_days_from_sunday();

        if self.minutes & (1 << minute) == 0 {
            return false;
        }
        if self.hours & (1 << hour) == 0 {
            return false;
        }
        if self.months & (1 << month) == 0 {
            return false;
        }
        let dom_hit = self.month_days & (1 << dom) != 0;
        let dow_hit = self.week_days & (1 << dow) != 0;
        // Vixie OR 语义：两者都受限时任一命中即可；否则两者都要满足
        match (self.dom_any, self.dow_any) {
            (false, false) => dom_hit || dow_hit,
            _ => dom_hit && dow_hit,
        }
    }

    /// 从 `from_secs`（UNIX 秒）之后找下一次触发时间（UNIX 毫秒），按设备本地时区解释。
    pub fn next_after(&self, from_secs: i64) -> Option<i64> {
        match resolve_timezone() {
            // 固定偏移：夏令时无关，直接走定偏移扫描
            Timezone::Fixed(off) => self.next_after_in(from_secs, off),
            // 回退 UTC：按 UTC 墙钟解释
            Timezone::Unknown => self.next_after_in(from_secs, 0),
            // OS 本地时区：偏移随夏令时变化，需在扫描途中刷新
            Timezone::OsLocal => self.next_after_local(from_secs),
        }
    }

    /// 按设备本地时区求下一次触发（UNIX 毫秒），扫描途中按 15 分钟粒度刷新偏移。
    ///
    /// 偏移刷新粒度取 15 分钟：时区偏移只可能是 15 分钟的整数倍（IANA 时区里
    /// 存在 +05:30 / +05:45 这类），夏令时切换点必然落在 15 分钟边界上，
    /// 因此该粒度既不会漏掉切换，也不会引入可见误差。
    /// 刷新而不是全程固定偏移，是为了夏令时区切换当天的触发时刻不错位。
    fn next_after_local(&self, from_secs: i64) -> Option<i64> {
        let mut t = (from_secs / 60 + 1) * 60;
        let end = from_secs + 366 * 86_400;
        let mut offset = os_local_offset_secs(t);
        let mut offset_slot = t / 900;
        while t <= end {
            let slot = t / 900;
            if slot != offset_slot {
                offset_slot = slot;
                offset = os_local_offset_secs(t);
            }
            if self.matches_in(t, offset) {
                return Some(t * 1000);
            }
            t += 60;
        }
        None
    }

    /// 按指定 UTC 偏移求下一次触发（UNIX 毫秒）。
    ///
    /// 逐分钟扫描，上限 366 天（约 52 万步，每步几次位运算，微秒级完成）。
    /// 表达式在一年内无命中（如 2 月 30 日）返回 None，上层按「任务永不触发」处理。
    ///
    /// 偏移取自扫描起点并固定：中国大陆无夏令时，现场无偏差；夏令时区跨切换日
    /// 最多偏 1 小时。
    pub fn next_after_in(&self, from_secs: i64, offset_secs: i32) -> Option<i64> {
        // 起点取下一分钟整
        let mut t = (from_secs / 60 + 1) * 60;
        let end = from_secs + 366 * 86_400;
        while t <= end {
            if self.matches_in(t, offset_secs) {
                return Some(t * 1000);
            }
            t += 60;
        }
        None
    }
}

// ────────────────────────────────────────────────────────────
// 时区解析
// ────────────────────────────────────────────────────────────

/// 显式时区环境变量：值为**固定 UTC 偏移**，优先级最高。
///
/// 与 `TZ` 的区别：`TZ` 是操作系统级设置，支持 IANA 时区名与夏令时，但依赖
/// tzdata；`GATEWAY_TZ` 只认固定偏移，不依赖任何时区数据库，精简镜像里也可用。
pub const TZ_ENV: &str = "GATEWAY_TZ";

/// 时区来源
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Timezone {
    /// 显式固定偏移（秒，东为正）
    Fixed(i32),
    /// 操作系统本地时区（`TZ` 环境变量或 `/etc/localtime`，含夏令时）
    OsLocal,
    /// 未能确定设备时区，回退 UTC
    Unknown,
}

/// `GATEWAY_TZ` 写法非法时告警（只报一次，避免每次求值刷日志）
static INVALID_TZ_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 设备时区未能确定时告警（只报一次）
static UNKNOWN_TZ_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 时区数据（tzdata）缺失时告警（只报一次）
static MISSING_TZDATA_WARNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 只告警一次（`flag` 已置位则跳过）
fn warn_once(flag: &std::sync::atomic::AtomicBool, msg: impl FnOnce() -> String) {
    if !flag.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!("{}", msg());
    }
}

/// 解析固定 UTC 偏移写法（`GATEWAY_TZ` 的取值）
///
/// 支持：`Z`、`UTC`、`GMT`、`+08:00`、`+0800`、`+8`、`UTC+8`、`GMT-3` 等。
/// 小时取 0..=14，分钟取 0..=59；超出范围视为非法。
fn parse_fixed_offset(spec: &str) -> Option<i32> {
    let raw = spec.trim();
    if raw.is_empty() {
        return None;
    }
    let upper = raw.to_ascii_uppercase();
    if upper == "Z" {
        return Some(0);
    }
    // 去掉 UTC/GMT 前缀（"UTC" / "GMT" 本身表示 0 偏移）
    let body = upper
        .strip_prefix("UTC")
        .or_else(|| upper.strip_prefix("GMT"))
        .map(str::trim)
        .unwrap_or(upper.as_str());
    if body.is_empty() {
        return Some(0);
    }
    let (sign, digits) = match body.as_bytes().first() {
        Some(b'+') => (1i32, &body[1..]),
        Some(b'-') => (-1i32, &body[1..]),
        _ => (1i32, body),
    };
    if digits.is_empty() {
        return None;
    }
    let (hours, minutes) = if let Some((h, m)) = digits.split_once(':') {
        (h.trim().parse::<i32>().ok()?, m.trim().parse::<i32>().ok()?)
    } else if digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit()) {
        (
            digits[..2].parse::<i32>().ok()?,
            digits[2..].parse::<i32>().ok()?,
        )
    } else {
        (digits.parse::<i32>().ok()?, 0)
    };
    if !(0..=14).contains(&hours) || !(0..=59).contains(&minutes) {
        return None;
    }
    Some(sign * (hours * 3600 + minutes * 60))
}

/// 判定设备时区来源（纯函数版本，便于单测覆盖各分支）
///
/// `gateway_tz` = `GATEWAY_TZ` 取值，`tz_env` = `TZ` 取值。
fn timezone_from(gateway_tz: Option<&str>, tz_env: Option<&str>) -> Timezone {
    if let Some(spec) = gateway_tz.map(str::trim).filter(|s| !s.is_empty()) {
        match parse_fixed_offset(spec) {
            Some(off) => return Timezone::Fixed(off),
            None => warn_once(&INVALID_TZ_WARNED, || {
                format!(
                    "{}={} 不是合法的固定 UTC 偏移（支持 +08:00 / UTC+8 / Z；时区名称请用 TZ 环境变量），已忽略",
                    TZ_ENV, spec
                )
            }),
        }
    }

    // 操作系统本地时区：TZ 显式设置时用它，否则用 /etc/localtime
    let tz_set = tz_env.map(str::trim).filter(|s| !s.is_empty());
    if let Some(name) = tz_set {
        if !zoneinfo_available(name) {
            warn_once(&MISSING_TZDATA_WARNED, || {
                format!(
                    "时区数据缺失：TZ={} 在 /usr/share/zoneinfo 等目录找不到对应文件，\
                     需安装 tzdata，否则时区会静默退化为 UTC",
                    name
                )
            });
        }
        return Timezone::OsLocal;
    }
    if os_timezone_available() {
        return Timezone::OsLocal;
    }
    warn_once(&UNKNOWN_TZ_WARNED, || {
        format!(
            "未能确定设备时区（TZ 未设置，且无 /etc/localtime 与 zoneinfo），\
             计划任务按 UTC 解释；请设置 TZ=Asia/Shanghai 或 {} = +08:00",
            TZ_ENV
        )
    });
    Timezone::Unknown
}

/// 读取环境变量并判定时区来源（进程内只解析一次）
///
/// 缓存的是**来源**而非偏移：偏移每次都重新查（`chrono::Local` 自行跟踪
/// `TZ` 变更与夏令时），因此运行期改 `TZ` 仍能生效。
fn resolve_timezone() -> Timezone {
    static SOURCE: std::sync::LazyLock<Timezone> = std::sync::LazyLock::new(|| {
        timezone_from(
            std::env::var(TZ_ENV).ok().as_deref(),
            std::env::var("TZ").ok().as_deref(),
        )
    });
    *SOURCE
}

/// 时区数据文件是否存在（`name` 为 IANA 时区名或 POSIX TZ 串）
///
/// POSIX TZ 串（如 `CST-8`）由 chrono 自行解析，不需要 zoneinfo 文件。
fn zoneinfo_available(name: &str) -> bool {
    if !name.contains('/') {
        return true;
    }
    #[cfg(unix)]
    {
        const DIRS: [&str; 4] = [
            "/usr/share/zoneinfo",
            "/share/zoneinfo",
            "/etc/zoneinfo",
            "/usr/share/lib/zoneinfo",
        ];
        DIRS.iter()
            .any(|d| std::path::Path::new(d).join(name).exists())
    }
    #[cfg(not(unix))]
    {
        // Windows/macOS 由系统 API 提供时区数据，不存在 zoneinfo 目录
        let _ = name;
        true
    }
}

/// 操作系统是否提供本地时区信息
///
/// Unix：需有 `/etc/localtime` 或 zoneinfo 目录，否则时区无法确定。
/// 其他平台：系统 API 始终提供，直接认为可用。
fn os_timezone_available() -> bool {
    #[cfg(unix)]
    {
        const DIRS: [&str; 4] = [
            "/usr/share/zoneinfo",
            "/share/zoneinfo",
            "/etc/zoneinfo",
            "/usr/share/lib/zoneinfo",
        ];
        std::path::Path::new("/etc/localtime").exists()
            || DIRS.iter().any(|d| std::path::Path::new(d).is_dir())
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 操作系统本地时区在给定时刻的偏移（秒，东为正）
fn os_local_offset_secs(ts_secs: i64) -> i32 {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts_secs, 0) {
        chrono::LocalResult::Single(dt) => dt.offset().local_minus_utc(),
        _ => 0,
    }
}

/// 设备时区在给定时刻相对 UTC 的偏移（秒，东为正）
fn device_offset_secs(ts_secs: i64) -> i32 {
    match resolve_timezone() {
        Timezone::Fixed(off) => off,
        Timezone::OsLocal => os_local_offset_secs(ts_secs),
        Timezone::Unknown => 0,
    }
}

/// 生效时区的人类可读描述（启动日志用，运维据此核对「每天 02:00」是哪儿的 02:00）
pub fn tz_description() -> String {
    let tz = resolve_timezone();
    match tz {
        Timezone::Fixed(off) => format!("固定偏移 UTC{}（来源 {}）", format_offset(off), TZ_ENV),
        Timezone::OsLocal => {
            let now = chrono::Utc::now().timestamp();
            let off = os_local_offset_secs(now);
            format!("{}（UTC{}）", os_tz_name(), format_offset(off))
        }
        Timezone::Unknown => "UTC（未能识别设备时区，已回退；请设置 TZ 或 GATEWAY_TZ）".to_string(),
    }
}

/// 操作系统时区名（`TZ` 优先，其次 `/etc/localtime` 软链目标）
fn os_tz_name() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim();
        if !tz.is_empty() {
            return tz.to_string();
        }
    }
    #[cfg(unix)]
    {
        if let Ok(target) = std::fs::read_link("/etc/localtime") {
            let s = target.to_string_lossy().to_string();
            if let Some(idx) = s.find("zoneinfo/") {
                return s[idx + "zoneinfo/".len()..].to_string();
            }
            return s;
        }
    }
    "系统本地时区".to_string()
}

/// 偏移格式化为 `+08:00` / `-05:30`
fn format_offset(off: i32) -> String {
    let sign = if off < 0 { '-' } else { '+' };
    let abs = off.abs();
    format!("{}{:02}:{:02}", sign, abs / 3600, (abs % 3600) / 60)
}

/// 便捷入口：解析并求下次触发（毫秒）。表达式非法或无命中返回 None。
pub fn next_run_ms(expr: &str, from_secs: i64) -> Option<i64> {
    parse(expr)?.next_after(from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用例统一按 UTC+8 墙钟构造 epoch，避免结果随开发机/CI 时区漂移
    const CST: i32 = 8 * 3600;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .and_then(|d| d.and_hms_opt(h, mi, s))
            .expect("valid date")
            .and_utc()
            .timestamp()
            - CST as i64
    }

    /// 把「本地时区」语义钉在 UTC+8，保证用例与运行环境时区无关
    trait CstExt {
        fn matches_cst(&self, ts_secs: i64) -> bool;
        fn next_after_cst(&self, from_secs: i64) -> Option<i64>;
    }

    impl CstExt for CronSchedule {
        fn matches_cst(&self, ts_secs: i64) -> bool {
            self.matches_in(ts_secs, CST)
        }
        fn next_after_cst(&self, from_secs: i64) -> Option<i64> {
            self.next_after_in(from_secs, CST)
        }
    }

    #[test]
    fn parse_every_5_min() {
        let s = parse("*/5 * * * *").expect("parse");
        // 命中整 5 分钟
        assert!(s.matches_cst(ts(2026, 10, 25, 10, 0, 0)));
        assert!(s.matches_cst(ts(2026, 10, 25, 14, 35, 0)));
        assert!(!s.matches_cst(ts(2026, 10, 25, 14, 36, 0)));
    }

    #[test]
    fn parse_daily_2am() {
        let s = parse("0 2 * * *").expect("parse");
        assert!(s.matches_cst(ts(2026, 10, 25, 2, 0, 0)));
        assert!(!s.matches_cst(ts(2026, 10, 25, 3, 0, 0)));
        assert!(!s.matches_cst(ts(2026, 10, 25, 2, 1, 0)));
    }

    #[test]
    fn parse_weekly_monday() {
        let s = parse("0 0 * * 1").expect("parse");
        // 2026-10-26 是周一
        assert!(s.matches_cst(ts(2026, 10, 26, 0, 0, 0)));
        assert!(!s.matches_cst(ts(2026, 10, 25, 0, 0, 0)));
    }

    #[test]
    fn sunday_0_and_7_both_mean_sunday() {
        let a = parse("0 0 * * 0").expect("p0");
        let b = parse("0 0 * * 7").expect("p7");
        assert_eq!(a, b);
        // 2026-10-25 是周日
        assert!(a.matches_cst(ts(2026, 10, 25, 0, 0, 0)));
    }

    #[test]
    fn lists_and_ranges() {
        let s = parse("0,30 9-18 * * *").expect("parse");
        assert!(s.matches_cst(ts(2026, 10, 25, 9, 0, 0)));
        assert!(s.matches_cst(ts(2026, 10, 25, 18, 30, 0)));
        assert!(!s.matches_cst(ts(2026, 10, 25, 19, 0, 0)));
        assert!(!s.matches_cst(ts(2026, 10, 25, 9, 15, 0)));
        // 步进区间
        let s2 = parse("1-10/3 * * * *").expect("parse2");
        assert!(s2.matches_cst(ts(2026, 10, 25, 0, 1, 0)));
        assert!(s2.matches_cst(ts(2026, 10, 25, 0, 4, 0)));
        assert!(!s2.matches_cst(ts(2026, 10, 25, 0, 2, 0)));
    }

    #[test]
    fn invalid_expressions_rejected() {
        assert!(parse("* * * *").is_none(), "字段数不足");
        assert!(parse("* * * * * *").is_none(), "字段数过多");
        assert!(parse("61 * * * *").is_none(), "分钟越界");
        assert!(parse("*/0 * * * *").is_none(), "步进为 0");
        assert!(parse("abc * * * *").is_none(), "非数字");
        assert!(parse("5-1 * * * *").is_none(), "区间倒置");
        assert!(parse("").is_none());
    }

    #[test]
    fn next_after_skips_to_next_boundary() {
        let s = parse("*/15 * * * *").expect("parse");
        // 10:07:30 → 下一次 10:15:00
        let next = s.next_after_cst(ts(2026, 10, 25, 10, 7, 30)).expect("next");
        assert_eq!(next, ts(2026, 10, 25, 10, 15, 0) * 1000);
    }

    #[test]
    fn next_after_daily_2am() {
        let s = parse("0 2 * * *").expect("parse");
        // 当天 10:00 → 次日 02:00
        let next = s.next_after_cst(ts(2026, 10, 25, 10, 0, 0)).expect("next");
        assert_eq!(next, ts(2026, 10, 26, 2, 0, 0) * 1000);
        // 当天 01:59 → 当天 02:00
        let next2 = s.next_after_cst(ts(2026, 10, 25, 1, 59, 0)).expect("next2");
        assert_eq!(next2, ts(2026, 10, 25, 2, 0, 0) * 1000);
    }

    #[test]
    fn dom_dow_or_semantics() {
        // Vixie：dom 与 dow 都受限时取并集——「每月 1 号 或 周一」
        let s = parse("0 0 1 * 1").expect("parse");
        // 2026-10-26 周一（非 1 号）
        assert!(s.matches_cst(ts(2026, 10, 26, 0, 0, 0)));
        // 2026-11-01（是 1 号；查日历：2026-11-01 是周日，非周一）
        assert!(s.matches_cst(ts(2026, 11, 1, 0, 0, 0)));
        // 普通周二且非 1 号
        assert!(!s.matches_cst(ts(2026, 10, 27, 0, 0, 0)));
    }

    #[test]
    fn never_matching_returns_none() {
        // 2 月 30 日永不命中
        let s = parse("0 0 30 2 *").expect("parse");
        assert!(s.next_after_cst(ts(2026, 1, 1, 0, 0, 0)).is_none());
    }

    #[test]
    fn matches_follows_device_timezone() {
        let s = parse("15 3 * * *").expect("parse");
        // 目标：设备时区墙钟为 2026-10-25 03:15 的那个时刻
        let target_wall = chrono::NaiveDate::from_ymd_opt(2026, 10, 25)
            .unwrap()
            .and_hms_opt(3, 15, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        // 反推 epoch：墙钟 - 设备偏移。偏移迭代一次以跨过夏令时切换边界。
        let probe = target_wall - device_offset_secs(target_wall) as i64;
        let t = target_wall - device_offset_secs(probe) as i64;
        assert!(s.matches(t), "应按设备时区的 03:15 命中");
        assert!(!s.matches(t + 3600), "设备时区 04:15 不应命中");
    }

    // ── 时区解析 ────────────────────────────────────────────
    // 以下用例只调用纯函数（`parse_fixed_offset` / `timezone_from`），
    // 不读写环境变量，因此与 CI 的 Timezone matrix（TZ=UTC / Asia/Shanghai /
    // America/New_York）互不干扰。

    #[test]
    fn fixed_offset_parsing() {
        assert_eq!(parse_fixed_offset("+08:00"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("+0800"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("+8"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("UTC+8"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("utc+8"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("GMT-3"), Some(-3 * 3600));
        assert_eq!(parse_fixed_offset("-05:30"), Some(-(5 * 3600 + 30 * 60)));
        assert_eq!(parse_fixed_offset("UTC"), Some(0));
        assert_eq!(parse_fixed_offset("Z"), Some(0));
        assert_eq!(parse_fixed_offset("  +02:00  "), Some(2 * 3600));
    }

    #[test]
    fn fixed_offset_rejects_invalid() {
        assert_eq!(parse_fixed_offset(""), None);
        assert_eq!(parse_fixed_offset("Asia/Shanghai"), None);
        assert_eq!(parse_fixed_offset("+15:00"), None, "超过 UTC+14");
        assert_eq!(parse_fixed_offset("+08:60"), None, "分钟越界");
        assert_eq!(parse_fixed_offset("abc"), None);
        assert_eq!(parse_fixed_offset("+"), None);
    }

    #[test]
    fn explicit_env_wins_over_os_timezone() {
        assert_eq!(
            timezone_from(Some("+08:00"), Some("America/New_York")),
            Timezone::Fixed(8 * 3600),
            "GATEWAY_TZ 应优先于 TZ"
        );
        // TZ 存在与否不影响显式偏移
        assert_eq!(
            timezone_from(Some("+08:00"), None),
            Timezone::Fixed(8 * 3600)
        );
    }

    #[test]
    fn invalid_explicit_offset_falls_back_to_os() {
        // 非法写法被忽略，退回操作系统时区（此处给了 TZ，结果应为 OsLocal）
        assert_eq!(
            timezone_from(Some("Asia/Shanghai"), Some("Asia/Shanghai")),
            Timezone::OsLocal
        );
    }

    #[test]
    fn blank_explicit_offset_is_ignored() {
        assert_eq!(
            timezone_from(Some("   "), Some("Asia/Shanghai")),
            Timezone::OsLocal,
            "空值不应被当作偏移"
        );
    }

    #[test]
    fn os_timezone_used_when_no_explicit_offset() {
        assert_eq!(
            timezone_from(None, Some("Asia/Shanghai")),
            Timezone::OsLocal
        );
    }

    #[test]
    fn fixed_offset_matches_use_explicit_wall_clock() {
        // 显式 UTC+8 时，「每天 02:00」= UTC 18:00，与运行环境的 TZ 无关
        let s = parse("0 2 * * *").expect("parse");
        let off = match timezone_from(Some("+08:00"), None) {
            Timezone::Fixed(off) => off,
            _ => panic!("应为固定偏移"),
        };
        // ts() 构造的是 UTC+8 墙钟对应的 epoch
        assert!(s.matches_in(ts(2026, 10, 25, 2, 0, 0), off));
        assert!(!s.matches_in(ts(2026, 10, 25, 3, 0, 0), off));
        // 同一时刻按 UTC 解释则不命中（这正是「差 8 小时」的形态）
        assert!(!s.matches_in(ts(2026, 10, 25, 2, 0, 0), 0));
    }

    #[test]
    fn offset_formatting() {
        assert_eq!(format_offset(8 * 3600), "+08:00");
        assert_eq!(format_offset(0), "+00:00");
        assert_eq!(format_offset(-(5 * 3600 + 30 * 60)), "-05:30");
    }
}
