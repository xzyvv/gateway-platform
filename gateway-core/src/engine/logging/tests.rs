use super::*;
use std::time::Duration;

/// 构造测试用文件元组：(文件名, 大小 MB, mtime 相对 UNIX_EPOCH 的秒偏移)
fn f(name: &str, size_mb: u64, mtime_offset_secs: u64) -> (PathBuf, u64, SystemTime) {
    (
        PathBuf::from(name),
        size_mb * 1024 * 1024,
        SystemTime::UNIX_EPOCH + Duration::from_secs(10000 + mtime_offset_secs),
    )
}

/// 空列表：无删除
#[test]
fn empty_files_no_deletion() {
    let files: Vec<(PathBuf, u64, SystemTime)> = vec![];
    let del = select_files_for_rotation(&files, 7, 50 * 1024 * 1024);
    assert_eq!(del.len(), 0);
}

/// 文件数 <= keep_recent：全部保留，即使超过阈值
#[test]
fn keeps_all_when_fewer_than_keep_recent() {
    let files = vec![f("active_big.log", 100, 1), f("active_big2.log", 100, 2)];
    let del = select_files_for_rotation(&files, 7, 50 * 1024 * 1024);
    assert_eq!(del.len(), 0, "keep_recent 内的文件不应被删，无论大小");
}

/// 超过 keep_recent 且超过阈值：删除
#[test]
fn deletes_old_files_above_threshold() {
    // 10 个 60MB 文件，按 mtime 递增；保留最新 7 个，删除最旧 3 个
    let files: Vec<_> = (0..10).map(|i| f(&format!("a{i}.log"), 60, i)).collect();
    let del = select_files_for_rotation(&files, 7, 50 * 1024 * 1024);
    assert_eq!(del.len(), 3, "第 8-10 个超过阈值的旧文件应被删");
    // 删除的应该是最旧的 3 个（a0/a1/a2）
    let names: Vec<_> = del
        .iter()
        .map(|(p, _)| p.to_string_lossy().to_string())
        .collect();
    assert!(names.contains(&"a0.log".to_string()));
    assert!(names.contains(&"a1.log".to_string()));
    assert!(names.contains(&"a2.log".to_string()));
}

/// 超过 keep_recent 但小于阈值：保留
#[test]
fn keeps_old_files_below_threshold() {
    let files: Vec<_> = (0..10).map(|i| f(&format!("a{i}.log"), 10, i)).collect();
    let del = select_files_for_rotation(&files, 7, 50 * 1024 * 1024);
    assert_eq!(del.len(), 0, "小于阈值的旧文件保留");
}

/// Bug 51 回归：大但新不应删，旧且大才删
///
/// 原实现按 size 排序，会保留小但旧的文件，误删大但新的活跃日志。
#[test]
fn bug51_large_new_kept_small_old_kept_large_old_deleted() {
    let files = vec![
        f("active_big.log", 100, 100), // 最新 + 最大：应保留
        f("old_small.log", 1, 1),      // 旧但小：保留（小于阈值）
        f("old_big.log", 60, 2),       // 旧且大：应删
    ];
    let del = select_files_for_rotation(&files, 1, 50 * 1024 * 1024);
    assert_eq!(del.len(), 1, "仅 old_big.log 应被删");
    assert_eq!(del[0].0, PathBuf::from("old_big.log"));
    assert_eq!(del[0].1, 60 * 1024 * 1024);
}

/// 相同 mtime：稳定排序保留原始顺序，仍按 size 过滤
#[test]
fn same_mtime_stable_sort_filters_by_size() {
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(10000);
    let files = vec![
        (PathBuf::from("a.log"), 60 * 1024 * 1024, base), // 第一个保留
        (PathBuf::from("b.log"), 60 * 1024 * 1024, base), // 超阈值删除
        (PathBuf::from("c.log"), 10 * 1024 * 1024, base), // 小于阈值保留
    ];
    let del = select_files_for_rotation(&files, 1, 50 * 1024 * 1024);
    assert_eq!(del.len(), 1);
    assert_eq!(del[0].0, PathBuf::from("b.log"));
}

/// 阈值边界：等于阈值的不删，仅严格大于才删
#[test]
fn boundary_equal_size_not_deleted() {
    let threshold: u64 = 50 * 1024 * 1024;
    let files = vec![
        f("keep.log", 10, 1),     // 保留（最新）
        f("equal.log", 50, 2),    // 等于阈值：不删
        f("over.log", 51, 3),     // 等于+1MB：不删（mtime 最新，被保留）
        f("over_old.log", 51, 0), // 旧且大于阈值：删
    ];
    let del = select_files_for_rotation(&files, 1, threshold);
    // 排序后最新的是 over.log(3)，保留 1 个；
    // 剩余按时间：keep.log(1)/equal.log(2)/over_old.log(0)
    // 仅 over_old.log 51MB > 50MB 删除
    assert_eq!(del.len(), 1);
    assert_eq!(del[0].0, PathBuf::from("over_old.log"));
}

/// keep_recent=0：所有超过阈值的旧文件都被删（即使最新）
#[test]
fn zero_keep_recent_deletes_all_above_threshold() {
    let files = vec![
        f("a.log", 60, 5),
        f("b.log", 60, 1),
        f("c.log", 10, 3), // 小于阈值，保留
    ];
    let del = select_files_for_rotation(&files, 0, 50 * 1024 * 1024);
    assert_eq!(del.len(), 2, "两个 60MB 文件都超过阈值，都应删");
    let names: Vec<_> = del
        .iter()
        .map(|(p, _)| p.to_string_lossy().to_string())
        .collect();
    assert!(names.contains(&"a.log".to_string()));
    assert!(names.contains(&"b.log".to_string()));
}

// ===== OP-5 (fix)：日志系统集成测试 =====
//
// init_logging 是全局副作用（tracing subscriber 单例），传统单元测试一次只能
// 跑一次，否则后续 try_init 静默 return false。本模块覆盖两个**纯函数/可多次跑**
// 路径：
// 1. select_files_for_rotation（之前已有 6 个测试）
// 2. 日志文件名前缀过滤（确保 #17 修复：只清理本系统日志，不误删目录内其他文件）

/// #17 回归：日志目录清理必须仅命中 `gateway.log*` 前缀，绝不动目录下的
/// 配置文件或 EULA 等"邻居文件"。
///
/// 反证闭环：把 `LOG_FILE_PREFIX` 改成 `""` 后，下方测试 fail——因为
/// `eula.txt`/`config.json` 也会被列入候选。
#[test]
fn log_rotation_only_targets_gateway_log_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 准备邻居文件 + gateway.log 文件
    std::fs::write(dir.path().join("eula.txt"), b"hi").unwrap();
    std::fs::write(dir.path().join("config.json"), b"{}").unwrap();
    std::fs::write(dir.path().join("gateway.log.20260101"), b"x").unwrap();
    std::fs::write(dir.path().join("gateway.log.20260102"), b"y").unwrap();

    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    let entries = std::fs::read_dir(dir.path()).expect("read dir");
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        // 完整复现 spawn_log_rotation 的候选列表构建逻辑——只保留 gateway.log 前缀
        if !name.starts_with(LOG_FILE_PREFIX) {
            continue;
        }
        let meta = e.metadata().unwrap();
        files.push((
            e.path(),
            meta.len(),
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        ));
    }

    // 候选列表里不应有 eula.txt / config.json
    let names: Vec<String> = files
        .iter()
        .map(|(p, _, _)| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().all(|n| n.starts_with(LOG_FILE_PREFIX)),
        "OP-5/#17: 候选必须 100% 以 gateway.log 前缀开头，实际列表: {names:?}"
    );
    assert_eq!(files.len(), 2, "OP-5: 应仅有 2 个 gateway.log.* 候选");
}

/// OP-5 (回归)：`GATEWAY_LOG_STDERR` 语义——未设置时默认开启 stderr 双输出，
/// 仅显式 false/0/no/off（大小写、空白不敏感）才关闭。
///
/// 反证闭环：若把 `stderr_enabled_from` 的默认值改成 `false`，
/// `stderr_layer_defaults_on_and_explicit_off_values` 第一处断言失败。
#[test]
fn stderr_layer_defaults_on_and_explicit_off_values() {
    assert!(
        stderr_enabled_from(None),
        "未设置时必须默认开启 stderr 双输出"
    );
    assert!(stderr_enabled_from(Some("true")), "显式 true 应开启");
    assert!(stderr_enabled_from(Some("1")), "1 视为开启");
    assert!(stderr_enabled_from(Some("yes")), "yes 视为开启");
    assert!(stderr_enabled_from(Some(" ON ")), "大小写与首尾空白不敏感");
    for off in ["false", "FALSE", "False", "0", "no", "off", "  Off  "] {
        assert!(
            !stderr_enabled_from(Some(off)),
            "GATEWAY_LOG_STDERR={off:?} 应关闭 stderr 层"
        );
    }
}

/// OP-5 契约：JSON 文件层必须真的产出**合法 JSON**，且经 non-blocking 管道后
/// 仍能落盘——`shutdown_logging()` 的 drop-guard 语义不得依赖进程退出。
///
/// 反证闭环：
/// 1) 若去掉 JSON 层的 `.json()`（改回人类可读格式），`serde_json::from_str` 解析失败；
/// 2) 若不 drop guard（保留原「静态量永不 Drop」行为），channel 中未落盘的行会丢，
///    `read_lines` 断言为空 → 失败。
#[cfg(not(feature = "tracing"))]
#[test]
fn json_file_layer_writes_parsable_json_and_flushes_on_guard_drop() {
    use tracing_subscriber::layer::SubscriberExt as _;

    let dir = tempfile::tempdir().expect("tempdir");
    let (layer, guard) = make_json_file_layer(dir.path()).expect("json file layer init");
    let subscriber = tracing_subscriber::registry().with(vec![layer]);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "op5_json_test", point_count = 7, "op5 json probe");
    });
    // 模拟 `shutdown_logging()` 的落盘动作：drop guard → worker 排空 channel
    drop(guard);

    let lines = read_log_lines(dir.path());
    assert_eq!(
        lines.len(),
        1,
        "OP-5: 单条事件应产生且仅产生一行 JSON，实际: {lines:?}"
    );
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap_or_else(|e| {
        panic!(
            "OP-5: 日志行必须是合法 JSON，解析失败 {e}；原文: {}",
            lines[0]
        )
    });
    assert_eq!(v["level"], "INFO", "OP-5: level 字段应为 INFO");
    assert_eq!(
        v["target"], "op5_json_test",
        "OP-5: target 字段应透出事件 target"
    );
    // 默认未开 `flatten_event`，事件字段统一挂在 `fields` 下（与 ELK/Loki 的
    // `fields.message` 解析约定一致）
    assert_eq!(
        v["fields"]["message"], "op5 json probe",
        "OP-5: message 应写入 fields"
    );
    assert_eq!(
        v["fields"]["point_count"], 7,
        "OP-5: 结构化字段值应保留且可被解析"
    );
}

/// OP-5 契约：`shutdown_logging()` 可重复调用且不 panic。
///
/// 停机路径有多条（正常停机 / 启动失败提前 return / 外层超时后强制退出），
/// 实现必须幂等——第二次调用时 guard 已被 `take()` 为 None，应安静返回。
#[test]
fn shutdown_logging_is_idempotent() {
    shutdown_logging();
    shutdown_logging();
}

/// OP-5 契约：被毒化的 `LOG_GUARD` Mutex 下 `shutdown_logging()` 必须不 panic。
///
/// 真实场景：生产中 supervisor/panic hook 一旦在某条路径上早于 shutdown 触发
/// panic，且 panic 时正持有 `LOG_GUARD`，标准库的 `parking_lot`/`std::Mutex`
/// 会把后续 `lock()` 返回 `Err(PoisonError)`。`shutdown_logging` 用 `lock().ok()`
/// 把这条路径**显式**降级为安静的跳过，但**没有测试作为反证**，下一任重构
/// 一旦有人把它改成 `unwrap()` 就立刻上线就会 panic。
///
/// 反证闭环：
/// 1) 把 [`shutdown_logging`] 里的 `.lock().ok()` 改回 `.lock().unwrap()` → 测试 fail；
/// 2) 把 `LOG_GUARD` 从 `std::sync::Mutex` 换成无 `.ok()` 替代品的实现 → 测试 fail。
#[test]
fn shutdown_logging_handles_poisoned_log_guard() {
    // 强制污染 LOG_GUARD：开一个 panic-guard 块，先抢锁、闭包内 panic，
    // Mutex 内部会标记 Poisoned，后续 lock() 返回 PoisonError。
    let _ = std::panic::catch_unwind(|| {
        let _g = LOG_GUARD.lock().unwrap();
        panic!("intentional panic to poison LOG_GUARD");
    });

    // 此时 LOG_GUARD 已 Poisoned —— shutdown_logging() 必须不 panic。
    // 即便此前 guard 已被 take()（它本身在 standard Mutex 下 take 与 poison
    // 各自独立；此测试的核心目的是验证 shutdown 的 .ok() 兜底）。
    shutdown_logging();
}

// #17 回归：日志文件前缀只匹配本系统日志，绝不伤及邻居文件。
// 见 [tests.rs] 顶部注释。
// 此处不再重复——已被 `log_rotation_only_targets_gateway_log_prefix` 覆盖。

// ===== P2-11：目录总量上限（第二轮清理）=====
//
// 此前的总量上限逻辑内联在 `spawn_log_rotation` 的 async 循环里：只能等一小时
// 在真机上观测，且删错文件（删掉正在写的活动日志）没人能提前发现。现在决策
// 收敛到纯函数 `plan_rotation_with`，下面用合成元数据覆盖三种关键分支。

/// 第一轮（大小阈值）已清干净：总量不超上限时，第二轮不得多删
#[test]
fn cap_pass_deletes_nothing_when_under_cap() {
    // 12 个 60MB：第一轮删掉最新 7 个之外的 5 个，剩 7 × 60MB = 420MB < 500MB
    let files: Vec<_> = (0..12).map(|i| f(&format!("a{i}.log"), 60, i)).collect();
    let del = plan_rotation(&files);
    assert_eq!(del.len(), 5, "超阈值且超出保留窗口的 5 个旧文件应被删");

    // 总量 420MB < 500MB，第二轮不应再删
    let cap: u64 = 500 * 1024 * 1024;
    let del2 = plan_rotation_with(&files, 7, 50 * 1024 * 1024, cap);
    assert_eq!(del2.len(), 5, "总量未超上限时不应触发第二轮删除");
}

/// 第二轮：全部文件都低于单文件阈值，但总量超上限 → 从最旧的开始删到达标
#[test]
fn cap_pass_deletes_oldest_first_until_under_cap() {
    // 10 × 60MB = 600MB，阈值设 100MB 让第一轮完全不动作
    let files: Vec<_> = (0..10).map(|i| f(&format!("a{i}.log"), 60, i)).collect();
    let cap = 200 * 1024 * 1024;
    let del = plan_rotation_with(&files, 3, 100 * 1024 * 1024, cap);

    // 需把 600MB 压到 ≤200MB：删最旧的前 7 个（420MB）后剩 180MB
    assert_eq!(del.len(), 7, "应删最旧的 7 个文件，实际删除列表: {del:?}");
    let names: Vec<String> = del
        .iter()
        .map(|(p, _)| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    for i in 0..7 {
        assert!(
            names.contains(&format!("a{i}.log")),
            "第二轮必须从最旧的开始删，缺少 a{i}.log；实际: {names:?}"
        );
    }
    // 最新 3 个必须原样保留
    for i in 7..10 {
        assert!(
            !names.contains(&format!("a{i}.log")),
            "最新 3 个文件不得被删，但 a{i}.log 在删除列表里"
        );
    }
}

/// 反证闭环：只保留 1 个文件且它自己就超过上限时，仍不得删它（正在写的活动文件）
#[test]
fn cap_pass_never_deletes_newest_even_if_it_alone_exceeds_cap() {
    let files = vec![f("only.log", 900, 1)];
    let del = plan_rotation_with(&files, 1, 1024 * 1024 * 1024, 500 * 1024 * 1024);
    assert!(del.is_empty(), "唯一（最新）文件即活动日志，绝不能被删");
}

/// 第二轮不得把第一轮已列入删除的文件重复计入（重复项会导致删除同一文件两次、
/// 日志里出现两条 "rotated" 记录，运维会误以为有第二个文件被清）
#[test]
fn cap_pass_does_not_duplicate_first_pass_entries() {
    // 10 × 10MB：全部低于单文件阈值 → 第一轮不动作；总量 100MB > 50MB 上限 → 第二轮动手
    let files: Vec<_> = (0..10).map(|i| f(&format!("a{i}.log"), 10, i)).collect();
    let del = plan_rotation_with(&files, 3, 50 * 1024 * 1024, 50 * 1024 * 1024);
    let mut names: Vec<String> = del
        .iter()
        .map(|(p, _)| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total, "删除列表存在重复项: {names:?}");
    assert_eq!(
        names,
        vec!["a0.log", "a1.log", "a2.log", "a3.log", "a4.log"],
        "应删最旧的 5 个（把 100MB 压到恰好 50MB 上限），最新 5 个保留"
    );
}

/// 端到端：`prune_log_dir` 真的删掉最旧的大文件，且不碰目录里的邻居文件
///
/// 反证闭环：把 `LOG_FILE_PREFIX` 改成空串（不再过滤前缀）→ 邻居文件被删、断言失败；
/// 把 `remove_file` 那行删掉（只计划不执行）→ 文件仍在、断言失败。
#[test]
fn prune_log_dir_deletes_old_big_log_and_keeps_neighbours() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("config.json"), b"{}").expect("write config");
    std::fs::write(dir.path().join("eula.txt"), b"hi").expect("write eula");

    // 9 个 gateway.log.*：最旧的 1 个 51MB（> 50MB 阈值），其余 8 个 1KB
    let old = dir.path().join("gateway.log.20260101");
    {
        use std::io::Write as _;
        let mut fh = std::fs::File::create(&old).expect("create big log");
        // 分块写入 51MB，避免一次分配大缓冲
        for _ in 0..51 {
            fh.write_all(&[b'x'; 1024 * 1024]).expect("write big log");
        }
    }
    for i in 0..8 {
        let p = dir.path().join(format!("gateway.log.2026010{}", i + 2));
        std::fs::write(&p, b"small").expect("write small log");
        // 显式设置 mtime：文件系统时间精度差异不应让测试结果不稳定
        let fh = std::fs::File::options()
            .write(true)
            .open(&p)
            .expect("open small log");
        fh.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(20_000 + i as u64))
            .expect("set mtime");
    }
    let fh = std::fs::File::options()
        .write(true)
        .open(&old)
        .expect("open big log");
    fh.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(10_000))
        .expect("set mtime");
    // 显式关闭句柄再清理：Windows 上「已被标记删除但仍有打开句柄」的文件，
    // `Path::exists()` 的返回值取决于句柄/扫描时机。全量并行测试（尤其 llvm-cov
    // 插桩让整体更慢）时曾因此偶发误报「大日志文件应已从磁盘移除」。
    // 轮转删的是归档文件，测试本身不该替它保持写句柄。
    drop(fh);

    let del = prune_log_dir(dir.path());

    assert!(
        del.iter().any(|(p, _)| p == &old),
        "最旧且超阈值的日志应被实际删除，实际删除列表: {del:?}"
    );
    assert!(!old.exists(), "大日志文件应已从磁盘移除");
    assert_eq!(del.len(), 1, "其余 8 个都在保留窗口内，不应被删");
    assert!(
        dir.path().join("config.json").exists() && dir.path().join("eula.txt").exists(),
        "日志目录里的邻居文件不得被清理"
    );
}

/// 读取临时日志目录下唯一的 `gateway.log*` 文件并按行收集（忽略空行）。
///
/// `RollingFileAppender` 的 DAILY 轮转会把文件名变成 `gateway.log.<date>`，
/// 故此处不硬编码日期，直接取目录内容。
#[cfg(not(feature = "tracing"))]
fn read_log_lines(dir: &std::path::Path) -> Vec<String> {
    let mut collected = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read log dir").flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(LOG_FILE_PREFIX)
        {
            continue;
        }
        let content = std::fs::read_to_string(entry.path()).expect("read log file");
        collected.extend(
            content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(str::to_string),
        );
    }
    collected
}
