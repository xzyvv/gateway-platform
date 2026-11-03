//! 内置模拟消费者（SimConsumer）API
//!
//! 模拟消费者是「无 MQTT 通道时的兜底消费者」：它把队列数据真的消费掉
//! （出队 → 处理 → ack）但不外发。因此它的运行状态必须**可观测、可控**——
//! 否则现场无法区分「队列在涨（没人消费）」与「队列在降（消费者在跑，
//! 数据被吃掉但没上报）」这两种完全相反的故障方向。
//!
//! 路由：见 [`routes::sim_consumer_routes`]
//! - GET  /api/sim_consumer/status  -> get_sim_consumer_status（读）
//! - POST /api/sim_consumer/start   -> start_sim_consumer（写）
//! - POST /api/sim_consumer/stop    -> stop_sim_consumer（写）

mod handlers;
mod routes;

pub(crate) use routes::sim_consumer_routes;
