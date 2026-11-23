// 本文件由 `npm run types:export`（cargo test export_ts_types_web_defaults）生成，请勿手改。
import type { MqttConfig, SimConsumerConfig, WebConfig } from '../types'

export const DEFAULT_WEB = {
  "api_token": "",
  "username": "admin",
  "password_hash": "",
  "listen_addr": "127.0.0.1:8080",
  "allow_anon_read": false,
  "realtime_buffer": 1024,
  "realtime_max_subscribers": 64,
  "ota_signing_key": null,
  "rate_limit_per_sec": null,
  "cors_origins": null,
  "allow_default_token": true,
  "users": [],
  "webhook_test_allow_hosts": [],
  "webhook_test_block_private": false,
  "demo_replay_sessions": false
} as unknown as WebConfig;
export const DEFAULT_MQTT = {
  "enabled": false,
  "broker": "tcp://localhost:1883",
  "topic": "factory/line1",
  "publish_interval": 5,
  "qos": 1,
  "keep_alive": 30,
  "ack_timeout_secs": null,
  "username": null,
  "password": null,
  "compression": null,
  "compression_threshold": null,
  "resume_rate_limit": null,
  "payload_template": null,
  "tls": null,
  "reconnect_backoff": {
    "seconds": [
      2,
      4,
      8,
      16,
      32,
      60
    ],
    "jitter_ratio": 0.4
  },
  "report_mode": "stream"
} as unknown as MqttConfig;
export const DEFAULT_SIM_CONSUMER = {
  "enabled": false,
  "batch_size": 100,
  "interval_ms": 1000,
  "log_every": 100
} as unknown as SimConsumerConfig;
export const DEFAULT_QUEUE: { max_rows: number } = {
  "max_rows": 1000000
};
export const DEFAULT_BUS_RECONNECT_BACKOFF: { seconds: number[]; jitter_ratio: number } = {
  "seconds": [
    1,
    2,
    5,
    10,
    30
  ],
  "jitter_ratio": 0.2
};
