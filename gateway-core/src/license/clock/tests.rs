use super::*;

fn guard(dir: &Path) -> ClockGuard {
    ClockGuard::new(&dir.join("clock.db"))
}

/// 辅助：创建一个"先于 now"的 license 文件，让 `no_license_anchor` fail-closed
/// 分支不会把"全新启动 + 无 license"误判为 Tampered。
fn make_license(dir: &Path) -> std::path::PathBuf {
    let lic = dir.join("test.lic");
    std::fs::write(&lic, b"license").unwrap();
    lic
}

/// 基准查询：直接读主副本，断言测试内部状态
fn db_last_seen(dir: &Path) -> Option<i64> {
    let g = guard(dir);
    g.read_db().unwrap().map(|r| r.last_seen)
}

#[test]
fn test_first_run_then_advance() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let g = guard(dir.path());
    // 取 license 文件 mtime 作为"now"基准——真实部署首启时 now ≈ mtime，
    // 此处用 1_000_000 这种固定值会被新 fail-closed 逻辑判 Tampered
    // （now << license 文件 mtime = 攻击者把时钟回拨到 license 之前）。
    let mtime = file_mtime_secs(&lic).unwrap();
    assert_eq!(
        g.check_and_update(mtime, Some(&lic)).unwrap(),
        ClockVerdict::FirstRun
    );
    // 时间正常前移
    assert_eq!(
        g.check_and_update(mtime + 100, Some(&lic)).unwrap(),
        ClockVerdict::Ok
    );
    assert_eq!(
        g.check_and_update(mtime + 200, Some(&lic)).unwrap(),
        ClockVerdict::Ok
    );
}

#[test]
fn test_rollback_within_threshold_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let g = guard(dir.path());
    // 用 mtime 锚定首启 now，避免回拨 1h 后跨过 license mtime
    let mtime = file_mtime_secs(&lic).unwrap();
    g.check_and_update(mtime, Some(&lic)).unwrap();
    // 回拨 1h：容忍（断电后 RTC 漂移属常态）
    assert_eq!(
        g.check_and_update(mtime - 3600, Some(&lic)).unwrap(),
        ClockVerdict::Ok
    );
}

#[test]
fn test_rollback_beyond_threshold_tampered() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let g = guard(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    g.check_and_update(mtime, Some(&lic)).unwrap();
    // 回拨 49h：判 Tampered
    assert_eq!(
        g.check_and_update(mtime - 49 * 3600, Some(&lic)).unwrap(),
        ClockVerdict::Tampered
    );
}

#[test]
fn test_ntp_resync_resets_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let g = guard(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    g.check_and_update(mtime, Some(&lic)).unwrap();
    assert_eq!(
        g.check_and_update(mtime - 49 * 3600, Some(&lic)).unwrap(),
        ClockVerdict::Tampered
    );
    // NTP 校时成功 → 重置基准到真实时间 → 旧时间不再触发 Tampered
    g.resync(mtime + 1_000_000).unwrap();
    assert_eq!(
        g.check_and_update(mtime + 999_000, Some(&lic)).unwrap(),
        ClockVerdict::Ok
    );
}

#[test]
fn test_table_persists_across_instances() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let db = dir.path().join("clock.db");
    // 全新启动：now=mtime+10 → FirstRun，基准锚定到 mtime+10
    ClockGuard::new(&db)
        .check_and_update(mtime + 10, Some(&lic))
        .unwrap();
    // 时间正常前移 1 小时：Ok，基准前进到 mtime+3610
    ClockGuard::new(&db)
        .check_and_update(mtime + 3610, Some(&lic))
        .unwrap();
    // 重建实例（模拟进程重启）后基准仍在
    let g2 = ClockGuard::new(&db);
    assert_eq!(
        g2.check_and_update(mtime + 7200, Some(&lic)).unwrap(),
        ClockVerdict::Ok
    );
    // 回拨 49h：判 Tampered
    assert_eq!(
        g2.check_and_update(mtime + 7200 - 49 * 3600, Some(&lic))
            .unwrap(),
        ClockVerdict::Tampered
    );
}

// ===== #18 多副本加固 =====

#[test]
fn test_replica_file_written_alongside_db() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let g = guard(dir.path());
    let expected = mtime + 1_000_000;
    g.check_and_update(expected, Some(&lic)).unwrap();
    // 两份副本都应存在
    assert!(dir.path().join("clock.db").exists());
    assert!(dir.path().join("clock.db.replica").exists());
    assert_eq!(g.read_replica().map(|r| r.last_seen), Some(expected));
}

#[test]
fn test_delete_db_does_not_reset_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let db = dir.path().join("clock.db");
    let g = ClockGuard::new(&db);
    g.check_and_update(mtime + 5_000_000, Some(&lic)).unwrap();

    // 攻击者删除主副本（SQLite 文件），但旁路副本仍在
    std::fs::remove_file(&db).unwrap();
    // 回拨 49h 仍应被检出（基准未被重置为 now）
    assert_eq!(
        g.check_and_update(mtime + 5_000_000 - 49 * 3600, Some(&lic))
            .unwrap(),
        ClockVerdict::Tampered
    );
    // 自愈：主副本被重写
    assert!(db.exists());
}

#[test]
fn test_delete_replica_does_not_reset_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let db = dir.path().join("clock.db");
    let g = ClockGuard::new(&db);
    g.check_and_update(mtime + 5_000_000, Some(&lic)).unwrap();

    // 攻击者删除旁路副本
    std::fs::remove_file(dir.path().join("clock.db.replica")).unwrap();
    assert_eq!(
        g.check_and_update(mtime + 5_000_000 - 49 * 3600, Some(&lic))
            .unwrap(),
        ClockVerdict::Tampered
    );
    // 自愈：旁路副本被重写
    assert!(dir.path().join("clock.db.replica").exists());
}

#[test]
fn test_all_replicas_deleted_with_rollback_caught_by_license_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("clock.db");
    let lic = dir.path().join("test.lic");
    std::fs::write(&lic, b"license").unwrap();

    let g = ClockGuard::new(&db);
    g.check_and_update(5_000_000, Some(&lic)).unwrap();

    // 攻击者清掉两份副本 + 把时钟回拨到 license 文件之前
    std::fs::remove_file(&db).unwrap();
    std::fs::remove_file(dir.path().join("clock.db.replica")).unwrap();
    // license 文件 mtime ≈ 真实当前时间；用一个远早于它的 now
    let mtime = file_mtime_secs(&lic).unwrap();
    assert_eq!(
        g.check_and_update(mtime - 49 * 3600, Some(&lic)).unwrap(),
        ClockVerdict::Tampered
    );
}

#[test]
fn test_all_replicas_deleted_sane_clock_is_first_run() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("clock.db");
    let lic = dir.path().join("test.lic");
    std::fs::write(&lic, b"license").unwrap();
    let mtime = file_mtime_secs(&lic).unwrap();

    // 全新安装：无副本，时钟不落后于 license 文件 → FirstRun（放行）
    let g = ClockGuard::new(&db);
    assert_eq!(
        g.check_and_update(mtime, Some(&lic)).unwrap(),
        ClockVerdict::FirstRun
    );
}

#[test]
fn test_baseline_never_written_below_license_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let lic = dir.path().join("test.lic");
    std::fs::write(&lic, b"license").unwrap();
    let mtime = file_mtime_secs(&lic).unwrap();
    let g = guard(dir.path());
    // 首启时 now 略早于 mtime（1h 内）→ 放行但基准锚定到 mtime
    g.check_and_update(mtime - 3600, Some(&lic)).unwrap();
    assert_eq!(db_last_seen(dir.path()), Some(mtime));
}

#[test]
fn test_write_count_is_monotonic() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let g = guard(dir.path());
    g.check_and_update(mtime + 1_000_000, Some(&lic)).unwrap();
    let w1 = g.read_db().unwrap().unwrap().writes;
    g.check_and_update(mtime + 1_000_100, Some(&lic)).unwrap();
    let w2 = g.read_db().unwrap().unwrap().writes;
    assert!(w2 > w1, "writes must increase: {} -> {}", w1, w2);
    // 两份副本写入计数一致
    assert_eq!(g.read_replica().unwrap().writes, w2);
}

#[test]
fn test_legacy_db_without_writes_column_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let db = dir.path().join("clock.db");
    // 模拟旧版本建的表（无 writes 列）——基准锚定到 4_000_000（远早于 license mtime）
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE license_clock (id INTEGER PRIMARY KEY CHECK(id = 1), last_seen INTEGER NOT NULL);
             INSERT INTO license_clock (id, last_seen) VALUES (1, 4000000);",
        )
        .unwrap();
    }
    let g = ClockGuard::new(&db);
    // 老库基准 4_000_000 < license mtime：now 仍 ≥ mtime 时 base_last=Some(4_000_000) 路径判回拨阈值
    // 走 now=mtime+10_000（远大于 4_000_000）→ Ok；回拨 49h → Tampered
    assert_eq!(
        g.check_and_update(mtime + 10_000, Some(&lic)).unwrap(),
        ClockVerdict::Ok
    );
    // 后续回拨到老基准 - 49h
    assert_eq!(
        g.check_and_update(4_000_000 - 49 * 3600, Some(&lic))
            .unwrap(),
        ClockVerdict::Tampered
    );
    assert_eq!(g.read_db().unwrap().unwrap().writes, 2);
}

#[test]
fn test_corrupt_replica_treated_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let g = guard(dir.path());
    g.check_and_update(mtime + 5_000_000, Some(&lic)).unwrap();
    // 旁路副本内容被写坏 → 视为缺失，但主副本基准仍在
    std::fs::write(dir.path().join("clock.db.replica"), b"garbage").unwrap();
    assert_eq!(g.read_replica(), None);
    assert_eq!(
        g.check_and_update(mtime + 5_000_000 - 49 * 3600, Some(&lic))
            .unwrap(),
        ClockVerdict::Tampered
    );
}

/// P0-4 严格化回归：仅旁路副本幸存 + 回拨 → Tampered（旁路副本被攻击者改写的标准路径）。
#[test]
fn test_replica_only_rollback_is_tampered() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let db = dir.path().join("clock.db");
    // 先建立基线（双副本都在）
    ClockGuard::new(&db)
        .check_and_update(mtime + 5_000_000, Some(&lic))
        .unwrap();
    // 攻击者：删主库（保留可改写的 .replica）
    std::fs::remove_file(&db).unwrap();
    // #19-2026：.replica 现在是 GWCK2 + HMAC，攻击者改写后 HMAC 校验失败
    // → 视作缺失 → replica_only 攻击检测分支 → Tampered
    assert_eq!(
        ClockGuard::new(&db)
            .check_and_update(mtime + 5_000_000 - 49 * 3600, Some(&lic))
            .unwrap(),
        ClockVerdict::Tampered
    );
}

/// #19-2026：GWCK2 副本被篡改 → 视作缺失 → fail-closed。
#[test]
fn test_replica_hmac_tamper_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let db = dir.path().join("clock.db");
    // 建立合法 GWCK2 副本
    ClockGuard::new(&db)
        .check_and_update(mtime + 5_000_000, Some(&lic))
        .unwrap();
    // 验证副本文件确实是 GWCK2 格式
    let replica_path = db.with_extension("db.replica");
    let body = std::fs::read_to_string(&replica_path).unwrap();
    assert!(body.starts_with("GWCK2 "), "must be GWCK2 format");
    // 攻击者：篡改 last_seen（但保留原 HMAC）
    let mut parts: Vec<&str> = body.split_whitespace().collect();
    parts[1] = "0"; // 改 last_seen
    let tampered = parts.join(" ");
    std::fs::write(&replica_path, tampered).unwrap();
    // 重新启动 ClockGuard（保留 sqlite 主库的合法 last_seen 5_000_000）
    // 期望：.replica 解析为 None（HMAC 校验失败），主库 last_seen 仍可信
    //        → verdict = Ok（主库基准生效）
    // 关键：HMAC 校验失败时 .replica 不被采信，攻击者改写无效
    let verdict = ClockGuard::new(&db)
        .check_and_update(mtime + 5_000_100, Some(&lic))
        .unwrap();
    assert_eq!(
        verdict,
        ClockVerdict::Ok,
        "主库基准生效，HMAC 校验失败的副本被忽略"
    );
}

/// #19-2026：GWCK1 老格式被读到 → 视作缺失（不信任内容）→ 安全升级。
#[test]
fn test_replica_legacy_gwck1_treated_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let lic = make_license(dir.path());
    let mtime = file_mtime_secs(&lic).unwrap();
    let db = dir.path().join("clock.db");
    // 模拟升级前现场遗留的 GWCK1 老副本
    let replica_path = db.with_extension("db.replica");
    std::fs::write(
        &replica_path,
        format!("GWCK1 {} {}\n", mtime + 5_000_000, 1_i64),
    )
    .unwrap();
    // 解析时 GWCK1 → 返回 None（不信任内容）→ upgrade 后 write_replica 写 GWCK2
    // 主库为空 → verdict = FirstRun（因为 base_last=None）
    assert_eq!(
        ClockGuard::new(&db)
            .check_and_update(mtime, Some(&lic))
            .unwrap(),
        ClockVerdict::FirstRun
    );
    // 升级后副本应是 GWCK2 格式
    let body = std::fs::read_to_string(&replica_path).unwrap();
    assert!(body.starts_with("GWCK2 "), "升级后副本应是 GWCK2 格式");
}

/// #19-2026：HMAC 常量时间比较——长度不同立即 false（不暴露长度信息）。
#[test]
fn test_hmac_constant_time_eq() {
    // 等长相同
    assert!(constant_time_eq(b"abc", b"abc"));
    // 等长不同
    assert!(!constant_time_eq(b"abc", b"abd"));
    // 长度不同
    assert!(!constant_time_eq(b"abc", b"abcd"));
    // 空对空
    assert!(constant_time_eq(b"", b""));
}

/// #19-2026：replica 内容里混入非 GWCK1/GWCK2 魔数 → 视作缺失。
#[test]
fn test_replica_unknown_magic_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("clock.db");
    let replica_path = db.with_extension("db.replica");
    std::fs::write(&replica_path, "garbage content\n").unwrap();
    let g = ClockGuard::new(&db);
    assert_eq!(g.read_replica(), None);
}
