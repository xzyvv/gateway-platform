//! 回放会话持久化存储（sessions.json）
//!
//! 设计：进程内 Mutex 缓存 + 磁盘 sessions.json 落盘。启动时从 sessions.json 加载
//! （文件不存在/损坏则空列表，不阻断启动）；保存/删除会话时原子写（tmp+rename，
//! 与 ConfigLoader 同手法，防写一半断电损坏）。
//!
//! 预置会话（"电力仪表#1 正常采集"、"超时重试场景"等）为 handler 内置返回，
//! 不在本 store 管理范围（用户可见，但不可删）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use once_cell::sync::Lazy;
use parking_lot::Once;

use crate::web::api::protocol_tools::handlers::packet_replay::ReplaySession;

fn sessions_path(config_dir: &Path) -> PathBuf {
    config_dir.join("replay_sessions.json")
}

/// 全局存储：进程启动后 lazy 初始化一次，之后所有 handler 共用同一实例。
static STORE: Lazy<Mutex<Vec<ReplaySession>>> = Lazy::new(|| Mutex::new(Vec::new()));
static LOADED: Once = Once::new();

/// 取全局存储的锁。
///
/// Mutex 中毒（某线程持锁期间 panic）不视为致命：存储内容是纯数据
/// （可整体从 sessions.json 重建的会话列表），取回中毒的 guard 继续用，
/// 避免一次 panic 让后续所有回放接口永久 500。
fn lock_store() -> MutexGuard<'static, Vec<ReplaySession>> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 从磁盘加载一次（幂等；文件不存在或解析失败则空列表）
pub fn ensure_loaded(config_dir: &Path) {
    let path = sessions_path(config_dir);
    let mut loaded = false;
    LOADED.call_once(|| {
        loaded = true;
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(sessions) = serde_json::from_str::<Vec<ReplaySession>>(&data) {
                let n = sessions.len();
                *lock_store() = sessions;
                tracing::info!(
                    "packet-replay: loaded {} session(s) from {}",
                    n,
                    path.display()
                );
            } else {
                tracing::warn!(
                    "packet-replay: {} corrupted, resetting to empty",
                    path.display()
                );
            }
        }
    });
    // call_once 返回后 path 可能已变（config 目录迁移等罕见场景），但 session 存储
    // 语义是"进程生命周期内的会话集合"，迁移进程重启即可，不做双写迁移。
    let _ = loaded;
}

/// 原子落盘：tmp + rename（跨文件系统时 fallback 到直接写）
fn persist(config_dir: &Path) {
    let path = sessions_path(config_dir);
    let sessions = lock_store().clone();
    let Ok(json) = serde_json::to_string_pretty(&sessions) else {
        tracing::error!("packet-replay: serialize failed");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(json.as_bytes()))
        .and_then(|_| std::fs::rename(&tmp, &path))
        .is_err()
    {
        // 跨盘 rename 可能失败，fallback 直接覆盖写
        let _ = std::fs::remove_file(&tmp);
        if let Err(e) = std::fs::write(&path, &json) {
            tracing::error!("packet-replay: write {} failed: {}", path.display(), e);
        }
    }
}

/// 列出全部持久化会话（预置由 handler 另行合并）
pub fn list() -> Vec<ReplaySession> {
    lock_store().clone()
}

/// 按 id 查找持久化会话（找不到返回 None；预置由 handler 另行查找）
pub fn get(id: &str) -> Option<ReplaySession> {
    lock_store().iter().find(|s| s.id == id).cloned()
}

/// 保存或更新（id 已存在则替换）
pub fn upsert(config_dir: &Path, session: ReplaySession) {
    let mut lock = lock_store();
    if let Some(existing) = lock.iter_mut().find(|s| s.id == session.id) {
        *existing = session;
    } else {
        lock.push(session);
    }
    drop(lock);
    persist(config_dir);
}

/// 按 id 删除（不存在则 no-op）
pub fn delete(config_dir: &Path, id: &str) {
    let mut lock = lock_store();
    let before = lock.len();
    lock.retain(|s| s.id != id);
    if lock.len() != before {
        drop(lock);
        persist(config_dir);
    } else {
        drop(lock);
    }
}
