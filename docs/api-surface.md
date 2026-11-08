# API 端点清单（自动生成）

> **禁止手工编辑**。本文件由 `scripts/gen_api_surface.py` 从路由源码生成，
> CI 以 `python3 scripts/gen_api_surface.py --check` 校验；修改路由后请在本地
> 执行 `python3 scripts/gen_api_surface.py` 并提交本文件。

**数据来源**：`gateway-api/src/web/mod.rs`（health 探针）+ `gateway-api/src/web/api/*/routes.rs`（各业务子域）。

**统计**：16 个路由组 / 144 条唯一路径 / 173 个「方法 + 路径」端点（其中 WebSocket 端点 2 个）。

**鉴权说明**：`token` = 始终需要 `Authorization: Bearer <token>`；`token（allow_anon_read=true 时可匿名）` = 匿名只读开关打开时可不带 token；
`admin / exec scope` = 需要管理员或 `exec` scope 的 API 密钥；`无` = 公开端点。

## `health` — 健康检查（探针，内联于 `web/mod.rs`）

入口函数：`build_router`；端点数：2

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/health/live` | 无 | `health_live` |
| GET | `/health/ready` | 无 | `health_ready` |

## `access` — 访问控制（密钥 / 限流 / 计划任务）

入口函数：`access_routes`；端点数：15

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/access/rate-rules` | token（`allow_anon_read=true` 时可匿名） | `list_rate_rules` |
| GET | `/api/access/rate-stats` | token（`allow_anon_read=true` 时可匿名） | `rate_stats` |
| GET | `/api/access/rate-logs` | token（`allow_anon_read=true` 时可匿名） | `rate_logs` |
| GET | `/api/access/api-keys` | token（`allow_anon_read=true` 时可匿名） | `list_api_keys` |
| GET | `/api/access/scheduled-tasks` | token（`allow_anon_read=true` 时可匿名） | `list_sched_tasks` |
| POST | `/api/access/rate-rules` | token | `create_rate_rule` |
| PUT | `/api/access/rate-rules/:id` | token | `update_rate_rule` |
| DELETE | `/api/access/rate-rules/:id` | token | `delete_rate_rule` |
| POST | `/api/access/api-keys` | token | `create_api_key` |
| PUT | `/api/access/api-keys/:id` | token | `update_api_key` |
| DELETE | `/api/access/api-keys/:id` | token | `delete_api_key` |
| POST | `/api/access/scheduled-tasks` | token | `create_sched_task` |
| PUT | `/api/access/scheduled-tasks/:id` | token | `update_sched_task` |
| DELETE | `/api/access/scheduled-tasks/:id` | token | `delete_sched_task` |
| POST | `/api/access/scheduled-tasks/:id/run` | token | `run_sched_task_now` |

## `audit` — 审计日志

入口函数：`audit_routes`；端点数：1

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/audit/logs` | token（`allow_anon_read=true` 时可匿名） | `list_audit_logs` |

## `auth` — 认证与会话

入口函数：`auth_routes`；端点数：3

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| POST | `/api/auth/login` | 无 | `login` |
| GET | `/api/auth/whoami` | token | `whoami` |
| POST | `/api/auth/password` | token | `change_password` |

## `config` — 配置域（config / project / buses）

入口函数：`config_routes`；端点数：16

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/buses/paused` | token（`allow_anon_read=true` 时可匿名） | `list_paused_buses` |
| POST | `/api/buses/:name/pause` | token | `pause_bus_collect` |
| POST | `/api/buses/:name/resume` | token | `resume_bus_collect` |
| POST | `/api/config/backup` | token | `backup_config` |
| PUT | `/api/config/import` | token | `import_config` |
| POST | `/api/config/reload` | token | `reload_config` |
| POST | `/api/config/validate` | token | `validate_config_report` |
| GET | `/api/project/export` | token | `export_project` |
| POST | `/api/project/import` | token | `import_project` |
| GET | `/api/config` | token（`allow_anon_read=true` 时可匿名） | `get_config` |
| GET | `/api/config/versions` | token（`allow_anon_read=true` 时可匿名） | `list_config_versions` |
| GET | `/api/config/versions/:id` | token（`allow_anon_read=true` 时可匿名） | `get_config_version` |
| GET | `/api/config/versions/:id/compare` | token（`allow_anon_read=true` 时可匿名） | `compare_config_versions` |
| POST | `/api/config` | token | `post_config` |
| POST | `/api/config/versions/:id/rollback` | token | `rollback_config_version` |
| POST | `/api/config/diff` | token | `config_diff` |

## `devices` — 设备域（CRUD / 运维 / IO / 模板）

入口函数：`devices_routes`；端点数：29

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/devices` | token（`allow_anon_read=true` 时可匿名） | `list_devices` |
| GET | `/api/devices/:id/points` | token（`allow_anon_read=true` 时可匿名） | `list_points` |
| POST | `/api/devices` | token | `create_device` |
| PUT | `/api/devices/:id` | token | `update_device` |
| DELETE | `/api/devices/:id` | token | `delete_device` |
| POST | `/api/devices/:id/points` | token | `create_point` |
| GET | `/api/device-groups` | token（`allow_anon_read=true` 时可匿名） | `list_groups` |
| GET | `/api/devices/assets` | token（`allow_anon_read=true` 时可匿名） | `list_assets` |
| GET | `/api/devices/:id/asset` | token（`allow_anon_read=true` 时可匿名） | `get_asset` |
| GET | `/api/devices/health` | token（`allow_anon_read=true` 时可匿名） | `list_health` |
| GET | `/api/devices/shadows` | token（`allow_anon_read=true` 时可匿名） | `list_shadows` |
| GET | `/api/devices/control/writable` | token（`allow_anon_read=true` 时可匿名） | `list_writable_points` |
| GET | `/api/devices/control/commands` | token（`allow_anon_read=true` 时可匿名） | `list_commands` |
| GET | `/api/devices/topology` | token（`allow_anon_read=true` 时可匿名） | `device_topology` |
| GET | `/api/devices/point-quality` | token（`allow_anon_read=true` 时可匿名） | `list_point_quality` |
| GET | `/api/devices/point-quality/stats` | token（`allow_anon_read=true` 时可匿名） | `point_quality_stats` |
| GET | `/api/devices/backoff` | token（`allow_anon_read=true` 时可匿名） | `list_point_backoff` |
| POST | `/api/device-groups` | token | `create_group` |
| PUT | `/api/device-groups/:id` | token | `update_group` |
| DELETE | `/api/device-groups/:id` | token | `delete_group` |
| POST | `/api/device-groups/:id/devices` | token | `add_device_to_group` |
| DELETE | `/api/device-groups/:id/devices/:did` | token | `remove_device_from_group` |
| PUT | `/api/devices/:id/asset` | token | `update_asset` |
| POST | `/api/devices/control/send` | token | `send_command` |
| GET | `/api/io/status` | token（`allow_anon_read=true` 时可匿名） | `io_status` |
| GET | `/api/templates` | token（`allow_anon_read=true` 时可匿名） | `list_templates` |
| POST | `/api/io/:index` | token | `control_io` |
| POST | `/api/templates` | token | `save_template` |
| DELETE | `/api/templates/:name` | token | `delete_template` |

## `diagnostics` — 诊断（含日志流 / exec）

入口函数：`diagnostics_routes`；端点数：13

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/diagnostics/bus-load` | token（`allow_anon_read=true` 时可匿名） | `list_bus_loads` |
| GET | `/api/diagnostics/bus-load/:bus` | token（`allow_anon_read=true` 时可匿名） | `get_bus_load` |
| GET | `/api/diagnostics/modbus-errors` | token（`allow_anon_read=true` 时可匿名） | `list_modbus_errors` |
| GET | `/api/diagnostics/modbus-errors/issues` | token（`allow_anon_read=true` 时可匿名） | `list_common_issues` |
| GET | `/api/diagnostics/stats` | token（`allow_anon_read=true` 时可匿名） | `get_stats_report` |
| GET | `/api/diagnostics/schedule-timeline` | token（`allow_anon_read=true` 时可匿名） | `get_schedule_timeline` |
| GET | `/api/diagnostics/logs/download` | token（`allow_anon_read=true` 时可匿名） | `download_logs` |
| GET | `/api/diagnostics/logs/archives` | token（`allow_anon_read=true` 时可匿名） | `list_log_archives` |
| POST | `/api/diagnostics/bus-load/refresh` | token | `refresh_bus_loads` |
| POST | `/api/diagnostics/task-diag/:point_id` | token | `diagnose_point` |
| GET | `/api/diagnostics/gateway` | token（`allow_anon_read=true` 时可匿名） | `get_gateway_diag` |
| GET | `/api/diagnostics/logs` | token（`allow_anon_read=true` 时可匿名） | `diagnostics_logs_ws` |
| POST | `/api/diagnostics/exec` | admin / exec scope | `diagnostics_exec` |

## `history` — 历史数据

入口函数：`history_routes`；端点数：2

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/history` | token（`allow_anon_read=true` 时可匿名） | `history` |
| GET | `/api/history/export` | token（`allow_anon_read=true` 时可匿名） | `history_export_csv` |

## `license` — 授权

入口函数：`license_routes`；端点数：2

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/license` | token（`allow_anon_read=true` 时可匿名） | `get_license` |
| POST | `/api/license/import` | token | `import_license` |

## `points` — 点位域（CRUD / 回收站 / 即时读写 / 虚拟点位 / 批量）

入口函数：`points_routes`；端点数：20

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/point-trash` | token（`allow_anon_read=true` 时可匿名） | `list_point_trash` |
| DELETE | `/api/point-trash` | token | `purge_all_point_trash` |
| DELETE | `/api/point-trash/:id` | token | `purge_point` |
| PUT | `/api/points/:id` | token | `update_point` |
| DELETE | `/api/points/:id` | token | `delete_point` |
| POST | `/api/points/:id/restore` | token | `restore_point` |
| GET | `/api/points/:id/read` | token | `point_read_now` |
| POST | `/api/point-tools/write` | token | `points_write_unified` |
| POST | `/api/point-tools/batch` | token | `points_batch` |
| GET | `/api/point-tools/virtual` | token（`allow_anon_read=true` 时可匿名） | `list_virtual_points` |
| POST | `/api/point-tools/virtual` | token | `create_virtual_point` |
| POST | `/api/point-tools/virtual/:id/recalc` | token | `recalc_virtual_point` |
| PUT | `/api/point-tools/virtual/:id` | token | `update_virtual_point` |
| DELETE | `/api/point-tools/virtual/:id` | token | `delete_virtual_point` |
| GET | `/api/point-tools/offset` | token（`allow_anon_read=true` 时可匿名） | `offset_list` |
| GET | `/api/point-tools/clone/devices` | token（`allow_anon_read=true` 时可匿名） | `clone_devices` |
| POST | `/api/point-tools/find-replace/search` | token | `find_replace_search` |
| POST | `/api/point-tools/find-replace/apply` | token | `find_replace_apply` |
| POST | `/api/point-tools/offset/apply` | token | `offset_apply` |
| POST | `/api/point-tools/clone/apply` | token | `clone_apply` |

## `protocol_tools` — 协议工具

入口函数：`protocol_tools_routes`；端点数：31

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/protocol-tools/auto-detect/run/:id` | token（`allow_anon_read=true` 时可匿名） | `get_auto_detect_result` |
| GET | `/api/protocol-tools/register-scan` | token（`allow_anon_read=true` 时可匿名） | `list_register_scan_results` |
| GET | `/api/protocol-tools/register-scan/:id` | token（`allow_anon_read=true` 时可匿名） | `get_register_scan_result` |
| POST | `/api/protocol-tools/auto-detect/run` | token | `start_auto_detect` |
| POST | `/api/protocol-tools/auto-detect/run/:id/stop` | token | `stop_auto_detect` |
| POST | `/api/protocol-tools/register-scan` | token | `start_register_scan` |
| DELETE | `/api/protocol-tools/register-scan/:id` | token | `delete_register_scan_result` |
| POST | `/api/protocol-tools/register-scan/:id/suggestions` | token | `generate_register_suggestions` |
| GET | `/api/protocol-tools/packet-capture/packets` | token（`allow_anon_read=true` 时可匿名） | `list_packets` |
| GET | `/api/protocol-tools/packet-replay/sessions` | token（`allow_anon_read=true` 时可匿名） | `list_replay_sessions` |
| GET | `/api/protocol-tools/packet-replay/sessions/:id` | token（`allow_anon_read=true` 时可匿名） | `get_replay_session` |
| POST | `/api/protocol-tools/packet-capture/clear` | token | `clear_packets` |
| POST | `/api/protocol-tools/packet-replay/sessions` | token | `save_replay_session` |
| DELETE | `/api/protocol-tools/packet-replay/sessions/:id` | token | `delete_replay_session` |
| POST | `/api/protocol-tools/packet-replay/sessions/:id/execute` | token | `execute_replay` |
| GET | `/api/protocol-tools/packet-editor/presets` | token（`allow_anon_read=true` 时可匿名） | `list_presets` |
| POST | `/api/protocol-tools/packet-editor/send` | token | `send_frame` |
| GET | `/api/protocol-tools/convert/rules` | token（`allow_anon_read=true` 时可匿名） | `list_convert_rules` |
| POST | `/api/protocol-tools/convert/rules` | token | `create_convert_rule` |
| PUT | `/api/protocol-tools/convert/rules/:id` | token | `update_convert_rule` |
| DELETE | `/api/protocol-tools/convert/rules/:id` | token | `delete_convert_rule` |
| POST | `/api/protocol-tools/convert/rules/:id/toggle` | token | `toggle_convert_rule` |
| GET | `/api/protocol-tools/simulator/registers` | token（`allow_anon_read=true` 时可匿名） | `list_sim_registers` |
| GET | `/api/protocol-tools/simulator/status` | token（`allow_anon_read=true` 时可匿名） | `get_simulator_status` |
| PUT | `/api/protocol-tools/simulator/registers/:addr` | token | `update_sim_register` |
| POST | `/api/protocol-tools/simulator/start` | token | `start_simulator` |
| POST | `/api/protocol-tools/simulator/stop` | token | `stop_simulator` |
| POST | `/api/protocol-tools/slave-scan` | token | `scan_slaves` |
| POST | `/api/protocol-tools/iec104/ioa-read` | token | `read_iec104_ioas` |
| GET | `/api/protocols` | token（`allow_anon_read=true` 时可匿名） | `list_protocols` |
| GET | `/api/protocols/:name/diag` | token（`allow_anon_read=true` 时可匿名） | `get_protocol_diag` |

## `quality` — 数据质量规则

入口函数：`quality_routes`；端点数：4

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/quality/rules` | token（`allow_anon_read=true` 时可匿名） | `list_quality_rules` |
| POST | `/api/quality/rules` | token | `create_quality_rule` |
| PUT | `/api/quality/rules/:id` | token | `update_quality_rule` |
| DELETE | `/api/quality/rules/:id` | token | `delete_quality_rule` |

## `realtime` — 实时数据 / WebSocket

入口函数：`realtime_routes`；端点数：1

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/realtime` | token（`allow_anon_read=true` 时可匿名） | `realtime_ws` |

## `sim_consumer` — 内置模拟消费者

入口函数：`sim_consumer_routes`；端点数：3

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/sim_consumer/status` | token（`allow_anon_read=true` 时可匿名） | `get_sim_consumer_status` |
| POST | `/api/sim_consumer/start` | token | `start_sim_consumer` |
| POST | `/api/sim_consumer/stop` | token | `stop_sim_consumer` |

## `system` — 系统（状态 / OTA / NTP / 指标 / 证书 / 看门狗 / 上行）

入口函数：`system_routes`；端点数：23

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/version` | token（`allow_anon_read=true` 时可匿名） | `get_version` |
| GET | `/api/status` | token（`allow_anon_read=true` 时可匿名） | `get_status` |
| GET | `/api/health/report` | token（`allow_anon_read=true` 时可匿名） | `get_health_report` |
| GET | `/api/ota/status` | token（`allow_anon_read=true` 时可匿名） | `ota_status` |
| GET | `/api/ntp/status` | token（`allow_anon_read=true` 时可匿名） | `ntp_status` |
| GET | `/api/metrics` | token（`allow_anon_read=true` 时可匿名） | `get_metrics` |
| GET | `/api/cellular/status` | token（`allow_anon_read=true` 时可匿名） | `cellular_status` |
| GET | `/api/passthrough/sessions` | token（`allow_anon_read=true` 时可匿名） | `passthrough_sessions` |
| GET | `/api/north/status` | token（`allow_anon_read=true` 时可匿名） | `north_status` |
| GET | `/api/north/dead-letters` | token（`allow_anon_read=true` 时可匿名） | `list_dead_letters` |
| GET | `/api/health/disk/history` | token（`allow_anon_read=true` 时可匿名） | `disk_health_history` |
| GET | `/api/watchdog/services` | token（`allow_anon_read=true` 时可匿名） | `list_services` |
| GET | `/api/watchdog/events` | token（`allow_anon_read=true` 时可匿名） | `list_heal_events` |
| GET | `/api/certificates` | token（`allow_anon_read=true` 时可匿名） | `list_certificates` |
| POST | `/api/ota/upload` | token | `ota_upload` |
| POST | `/api/ota/rollback` | token | `ota_rollback` |
| POST | `/api/ntp/sync` | token | `ntp_sync` |
| POST | `/api/watchdog/services/:name/restart` | token | `restart_service` |
| POST | `/api/certificates/upload` | token | `upload_certificate` |
| POST | `/api/certificates/renew` | token | `renew_certificate` |
| POST | `/api/webhook/test` | token | `test_webhook` |
| POST | `/api/north/dead-letters/:id/replay` | token | `replay_dead_letter` |
| DELETE | `/api/north/dead-letters` | token | `purge_dead_letters` |

## `uplink` — 上行配置与上送测试

入口函数：`uplink_routes`；端点数：8

| 方法 | 路径 | 鉴权 | 处理器 |
|---|---|---|---|
| GET | `/api/uplink/config` | token（`allow_anon_read=true` 时可匿名） | `get_uplink_config` |
| GET | `/api/uplink/vars` | token（`allow_anon_read=true` 时可匿名） | `list_template_vars` |
| GET | `/api/uplink/template/export` | token（`allow_anon_read=true` 时可匿名） | `export_template` |
| GET | `/api/uplink/test/history` | token（`allow_anon_read=true` 时可匿名） | `list_test_history` |
| POST | `/api/uplink/config` | token | `save_uplink_config` |
| POST | `/api/uplink/test` | token | `test_uplink` |
| POST | `/api/uplink/template/import` | token | `import_template` |
| DELETE | `/api/uplink/test/history` | token | `clear_test_history` |

---

> 端点分组与鉴权层级见 [`architecture/protocols.md`](architecture/protocols.md#51-前后端通信)。
