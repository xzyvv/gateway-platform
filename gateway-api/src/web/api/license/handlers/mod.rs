//! License API 处理函数（授权状态查询 + 授权文件导入）

use super::*;

/// GET /api/license
pub(crate) async fn get_license(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let usage = build_usage(&state).await;
    Json(build_snapshot(&state, usage))
}

/// POST /api/license/import：导入授权文件（dry-run 通过才落盘）
pub(crate) async fn import_license(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportRequest>,
) -> Response {
    let mgr = &state.platform.license;
    match mgr.import(&req.content) {
        Ok(()) => {
            let s = mgr.current();
            // 授权档次/配额变化直接影响可用功能，导入成功必须留痕
            state.platform.audit.log(
                "license.import",
                "gateway",
                &format!(
                    "tier={:?} customer={}",
                    s.tier,
                    s.customer.as_deref().unwrap_or("-")
                ),
                "api",
            );
            let usage = build_usage(&state).await;
            // 导入低档位 license 后，存量配置可能已超出新档位配额。
            // 此时**不能阻断导入**（license 已经生效，配额是既定事实），
            // 但必须留痕 + 在响应里给出告警，否则用户要到下次保存配置才收到 4021。
            let exceeded = s.quota.exceeded_kinds(&usage);
            if !exceeded.is_empty() {
                let detail = exceeded
                    .iter()
                    .map(|k| k.label_zh())
                    .collect::<Vec<_>>()
                    .join("、");
                tracing::warn!(
                    "导入 license 后存量配置已超出新档位配额: tier={:?} 超限项=[{}]",
                    s.tier,
                    detail
                );
                state.platform.audit.log(
                    "license.quota_exceeded",
                    "gateway",
                    &format!("tier={:?} 超限项=[{}]", s.tier, detail),
                    "api",
                );
            }
            Json(build_snapshot(&state, usage)).into_response()
        }
        Err(e) => import_error(&e).into_response(),
    }
}

/// 组装授权快照 JSON：授权状态 + 设备码 + 配额用量 / 剩余量。
///
/// `usage` 的口径与配额校验**同源**（复用 `config_apply::count_*`），
/// 保证「已用 / 剩余」与「超限判定」不会出现两套算法互相打架。
fn build_snapshot(state: &Arc<AppState>, usage: QuotaUsage) -> serde_json::Value {
    let mgr = &state.platform.license;
    let s = mgr.current();
    let available = s.quota.available(&usage);
    LicenseSnapshot::to_json_value(&s, mgr.machine(), &usage, &available)
}

/// 当前生效配置的配额用量（口径同 `check_config_quota`）。
async fn build_usage(state: &Arc<AppState>) -> QuotaUsage {
    let cfg = state.config.load_full();
    QuotaUsage {
        points: crate::web::config_apply::count_points(state, &cfg),
        protocols: crate::web::config_apply::count_protocols(state, &cfg),
        storage_days: storage_days_usage(state).await,
        northbound_channels: crate::web::config_apply::count_northbound_channels(&cfg),
    }
}

/// `storage_days` 配额的已用量：队列中实际保留的历史数据跨度（天）。
///
/// 口径 = now - 最旧一条持久化数据的时间戳，向上取整到天。
/// 队列无历史数据时为 0。查询失败时按 0 计（统计失败不该阻断授权查询）。
async fn storage_days_usage(state: &Arc<AppState>) -> u32 {
    let Some(oldest) = state.south.queue.oldest_data_timestamp().await else {
        return 0;
    };
    let now = chrono::Utc::now().timestamp_millis();
    let span_ms = now.saturating_sub(oldest).max(0);
    // 向上取整：不足一天的历史也算占 1 天
    ((span_ms + 86_400_000 - 1) / 86_400_000) as u32
}

/// LicenseError → ApiError：按**失败性质**分类，而不是一律 400「参数不合法」。
///
/// 导入接口的输入是客户贴进来的 .lic 文本，失败分三类：
///
/// | 失败性质 | 码 | 典型场景 |
/// |---|---|---|
/// | 客户端提交的内容非法 | 4000 | 格式错 / 签名不符 / 设备码不符 / 版本不支持 |
/// | 服务端写盘失败 | 9999 | 工作目录无写权限 / 磁盘故障 |
/// | 授权文件本身已过期 | 7003 | 供应商签发时就已过期 |
///
/// 关键区分：**「内容非法」是 bad request（400）**——HTTP 语义上就是"你提交的东西不对"，
/// 现场看 400 才不会误以为是权限问题。旧实现一律 400 是对的，但**文案**错：
/// 统一说"请求参数不合法"，把"授权文件签名无效"说成"参数错了"，排查方向被带偏。
/// 现按失败原因给出可读中文文案。
///
/// 注：`dry_parse` 只做验签 + 指纹比对，**不查有效期**，所以 Expired 在导入路径
/// 实际不可达（过期文件导入后落盘，由 `current()` 呈现降级态）。此分支保留以防
/// 将来 dry_parse 增加有效期校验。
fn import_error(e: &LicenseError) -> ApiError {
    let msg = import_error_message(e);
    match e {
        // 客户端提交的内容非法 → bad request
        LicenseError::Decode
        | LicenseError::BadSignature
        | LicenseError::BadVersion(_)
        | LicenseError::BadPayload(_)
        | LicenseError::Fingerprint => ApiError::bad_request(msg),
        // 服务端环境问题（写盘失败）→ 不是客户的问题
        LicenseError::Io(_) => ApiError::internal(msg),
        // 授权文件已过期 → 授权失效码（覆写文案，保留"已过期"的具体说明）
        LicenseError::Expired => {
            let mut err = ApiError::license_expired();
            err.message = msg;
            err
        }
        _ => ApiError::bad_request(msg),
    }
}

/// LicenseError → 客户可读的中文提示（导入界面直接展示，不暴露内部错误码）
fn import_error_message(e: &LicenseError) -> String {
    match e {
        LicenseError::BadSignature => {
            "授权文件签名无效：文件可能被修改或损坏，请联系供应商重新签发".into()
        }
        LicenseError::Decode | LicenseError::BadVersion(_) | LicenseError::BadPayload(_) => {
            "授权文件格式错误：请勿用文本编辑器修改授权文件，请联系供应商重新签发".into()
        }
        LicenseError::Fingerprint => {
            "授权文件与本设备不匹配：请核对本机设备码后联系供应商重新签发".into()
        }
        LicenseError::Io(_) => "授权文件写入失败：请检查网关工作目录的写入权限".into(),
        LicenseError::Expired => "授权已过期：文件已导入，请向供应商申请续期授权".into(),
        other => format!("导入失败：{other}"),
    }
}
