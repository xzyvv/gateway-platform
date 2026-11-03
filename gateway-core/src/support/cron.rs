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
        self.matches_in(ts_secs, local_offset_secs(ts_secs))
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
        self.next_after_in(from_secs, local_offset_secs(from_secs))
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

/// 设备本地时区相对 UTC 的偏移（秒）。取不到时回退 UTC。
fn local_offset_secs(ts_secs: i64) -> i32 {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts_secs, 0) {
        chrono::LocalResult::Single(dt) => dt.offset().local_minus_utc(),
        _ => 0,
    }
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
    fn matches_follows_device_local_timezone() {
        use chrono::TimeZone;
        let s = parse("15 3 * * *").expect("parse");
        // 取本地墙钟 03:15 的时刻；该时刻在常见时区均无夏令时歧义
        let local = chrono::Local
            .with_ymd_and_hms(2026, 10, 25, 3, 15, 0)
            .single()
            .expect("本地时间无歧义");
        assert!(
            s.matches(local.timestamp()),
            "应按设备本地时区的 03:15 命中"
        );
        assert!(!s.matches(local.timestamp() + 3600), "本地 04:15 不应命中");
    }
}
