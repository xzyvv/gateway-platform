# 关键设计约束

> Gateway Platform 文档子页：原 [architecture.md](../architecture.md) 第 9 节独立成页，便于按主题检索与维护。

---

## 9. 关键设计约束

| 约束 | 说明 |
|---|---|
| 配置加载失败必须返回错误 | 不可静默忽略，确保首次落盘可观测 |
| OTA 升级必须检查状态并加锁 | 禁止并发写同一分区 |
| 磁盘满入队检查 `disk_full` + 退避 | 未到退避期直接丢弃，防 CPU 浪费 |
| Web 默认 `127.0.0.1:8080` | 默认仅回环；网线直连调试需显式改 `0.0.0.0:8080` 并先设置 `password_hash`（否则 fail-closed 拒绝启动） |
| `api_token` 为空自动生成随机 token | 保障鉴权安全 |
| 前端产物 500KB–1MB（gzip） | 适应 ARM CPU / 256MB–2GB 内存 |
| 前端离线可用，无 CDN | 直连调试无外网场景 |
| 协议插件化 | 新增协议只需 `ProtocolRegistry::register` |
| 采集与处理解耦 | mpsc 通道，采集周期不受边缘处理耗时影响 |

---

## 10. 锁与同步原语使用准则

项目使用 4 种同步原语，各有适用场景。新代码必须遵循以下准则：

### ArcSwap — 只读快照热更新
- 适用：配置热更新、需要无锁读取的共享不可变数据
- 特点：读取无锁（`load_full` 返回 `Arc` 快照），写入用 CAS 原子替换
- 项目实例：`AppState.config.config: ArcSwap<GatewayConfig>`
- 禁止：用于需要写入后立即读取最新值的场景（应用 `Mutex`）

### parking_lot::Mutex — 短临界区同步
- 适用：临界区内无 `.await`、持锁时间短的同步场景
- 特点：不中毒（panic 后仍可用）、性能优于 `std::Mutex`
- 项目实例：`Gateway.bus_managers`、`Gateway.mqtt`
- 禁止：临界区内有 `.await`（会阻塞 runtime worker 线程）

### tokio::sync::Mutex — 需要 await 持锁
- 适用：临界区内需要 `.await` 的异步场景
- 特点：async 感知，持锁期间不阻塞 runtime
- 项目实例：`Gateway.reload_lock`、`AppState.config.config_reload_lock`
- 禁止：短临界区（相比 `parking_lot` 有额外开销）

### parking_lot::RwLock — 读多写少
- 适用：读操作远多于写操作的共享可变状态
- 特点：读锁可并发，写锁独占
- 项目实例：`pipeline::point_cache::PointCache`、`north::mqtt` 点位配置快照
- 禁止：写操作频繁（写锁饥饿读锁）

### 选择决策树
```
需要热更新只读快照？→ ArcSwap
临界区内有 .await？  → tokio::sync::Mutex
读远多于写？         → parking_lot::RwLock
其他短临界区？        → parking_lot::Mutex
```