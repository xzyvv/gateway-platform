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

/// 回放会话保留期：30 天（DD-26）。
///
/// 会话只是调试辅助数据，长期堆积既占磁盘也让列表越来越难找。过期采用
/// 「读/写时惰性清理」而非后台定时器：TTL 精度要求天级，没必要为它常驻任务。
pub const REPLAY_SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 移除超过保留期的会话，返回是否真的有删除（供调用方决定是否落盘）。
fn purge_expired(lock: &mut Vec<ReplaySession>, now: i64) -> bool {
    let before = lock.len();
    lock.retain(|s| s.created_at_ms.saturating_add(REPLAY_SESSION_TTL_MS) > now);
    lock.len() != before
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

/// 列出全部持久化会话（预置由 handler 另行合并）；顺带清理过期会话。
pub fn list(config_dir: &Path) -> Vec<ReplaySession> {
    purge_and_collect(config_dir)
}

/// 按 id 查找持久化会话（找不到返回 None；预置由 handler 另行查找）。
/// 查找前同样先清理过期会话，避免「列表已看不到、按 id 还能取到」。
pub fn get(config_dir: &Path, id: &str) -> Option<ReplaySession> {
    purge_and_collect(config_dir)
        .into_iter()
        .find(|s| s.id == id)
}

fn purge_and_collect(config_dir: &Path) -> Vec<ReplaySession> {
    let mut lock = lock_store();
    let removed = purge_expired(&mut lock, now_ms());
    let snapshot = lock.clone();
    drop(lock);
    if removed {
        persist(config_dir);
    }
    snapshot
}

/// 保存或更新（id 已存在则替换）
pub fn upsert(config_dir: &Path, session: ReplaySession) {
    let mut lock = lock_store();
    purge_expired(&mut lock, now_ms());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, created_at_ms: i64) -> ReplaySession {
        ReplaySession {
            id: id.into(),
            name: id.into(),
            device_name: id.into(),
            duration_ms: 0,
            packet_count: 0,
            created_at_ms,
            owner: Some("admin".into()),
            packets: Vec::new(),
        }
    }

    #[test]
    fn ttl_只清理过期会话_到期即删() {
        let now = 1_000_000_000_000i64;
        let mut list = vec![
            session("fresh", now),
            // 恰好满 30 天即视为过期（保留期不含边界时刻）
            session("edge", now - REPLAY_SESSION_TTL_MS),
            session("expired", now - REPLAY_SESSION_TTL_MS - 1),
        ];
        assert!(purge_expired(&mut list, now), "有删除应返回 true");
        let ids: Vec<&str> = list.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["fresh"]);
    }

    #[test]
    fn ttl_无过期时不写盘() {
        let now = 1_000_000_000_000i64;
        let mut list = vec![session("a", now), session("b", now - 10)];
        assert!(!purge_expired(&mut list, now), "无删除不得触发落盘");
        assert_eq!(list.len(), 2);
    }
}
