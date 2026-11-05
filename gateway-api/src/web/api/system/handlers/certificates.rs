//! 证书管理 API
//!
//! 基于 CertManager 暴露网关 X.509 证书的查看、上传替换、重新生成。
//!
//! 路由：
//! - GET  /api/certificates        -> list_certificates
//! - POST /api/certificates/upload -> upload_certificate
//! - POST /api/certificates/renew  -> renew_certificate

use super::*;
use x509_parser::prelude::*;

/// 证书信息（与前端 Certificate 类型对齐）
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CertificateInfo {
    pub id: String,
    pub name: String,
    #[ts(type = "'server' | 'client' | 'ca'")]
    pub cert_type: String,
    pub subject: String,
    pub issuer: String,
    pub serial_number: String,
    pub valid_from: i64,
    pub valid_to: i64,
    /// 剩余有效天数（负数 = 已过期）；第 1 期冗余收口：
    /// 原独立端点 `/api/cert/status` 的字段并入本列表。
    pub days_remaining: i64,
    /// 证书文件在本机的路径
    pub cert_path: String,
    pub fingerprint: String,
    pub enabled: bool,
    pub bound_to: Option<String>,
}

/// 上传证书请求
#[derive(serde::Deserialize)]
pub struct UploadCertRequest {
    pub name: String,
    pub cert_pem: String,
    pub key_pem: String,
}

/// GET /api/certificates — 证书列表
///
/// 当前网关仅维护一对证书（gateway.crt / gateway.key），
/// 以单元素列表返回，兼容前端多证书展示结构。
pub(crate) async fn list_certificates(
    State(state): State<Arc<AppState>>,
) -> Result<ApiResponse<Vec<CertificateInfo>>, ApiError> {
    let mgr = &state.north.cert_manager;
    match mgr.load() {
        Ok(pair) => {
            let parsed = X509Certificate::from_der(&pair.cert_der);
            let (subject, issuer, valid_from, valid_to, serial, fingerprint) = match parsed {
                Ok((_rem, cert)) => {
                    let subject = cert.subject().to_string();
                    let issuer = cert.issuer().to_string();
                    let valid_from = cert.validity().not_before.to_datetime().unix_timestamp();
                    let valid_to = cert.validity().not_after.to_datetime().unix_timestamp();
                    let serial = cert.raw_serial_as_string();
                    let digest = ring::digest::digest(&ring::digest::SHA256, &pair.cert_der);
                    let fp = digest
                        .as_ref()
                        .iter()
                        .map(|b| format!("{:02X}", b))
                        .collect::<Vec<_>>()
                        .join(":");
                    (subject, issuer, valid_from, valid_to, serial, fp)
                }
                Err(e) => {
                    // 5001：证书解析失败（服务端错误，区间映射为 HTTP 500）
                    return Ok(ApiResponse::err(5001, format!("parse cert failed: {}", e)));
                }
            };

            // `bound_to` 必须反映真实消费者，否则运维会按错误信息排查：
            // 本网关的 Web 是明文 HTTP（TLS 由外部反代承担），这张证书实际只
            // 用于 MQTT 客户端 mTLS 与内置 OPC UA Server。旧实现硬编码
            // "mqtt,https" 会让人误以为 Web 也在用它。
            let cfg = state.config.config.load();
            let mut consumers: Vec<&str> = Vec::new();
            let mqtt_encrypted = ["tls://", "mqtts://", "ssl://", "wss://"]
                .iter()
                .any(|p| cfg.mqtt.broker.starts_with(p));
            if cfg.mqtt.enabled && mqtt_encrypted {
                consumers.push("mqtt");
            }
            if cfg.opcua.as_ref().map(|o| o.enabled).unwrap_or(false) {
                consumers.push("opcua");
            }
            let bound_to = if consumers.is_empty() {
                None
            } else {
                Some(consumers.join(","))
            };

            let cert = CertificateInfo {
                id: "gateway".to_string(),
                name: "网关 TLS 证书".to_string(),
                cert_type: "server".to_string(),
                subject,
                issuer,
                serial_number: serial,
                valid_from,
                valid_to,
                days_remaining: (valid_to - chrono::Utc::now().timestamp()) / 86400,
                cert_path: mgr.cert_path().display().to_string(),
                fingerprint,
                enabled: true,
                bound_to,
            };
            Ok(ApiResponse::ok(vec![cert]))
        }
        Err(_e) => Ok(ApiResponse::ok(Vec::new())),
    }
}

/// POST /api/certificates/upload — 上传替换证书
pub(crate) async fn upload_certificate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UploadCertRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let mgr = &state.north.cert_manager;
    // 写入证书和私钥文件
    let cert_path = mgr.cert_path().to_path_buf();
    let key_path = mgr.key_path().to_path_buf();
    tokio::fs::write(&cert_path, &req.cert_pem)
        .await
        .map_err(|e| ApiError::internal(format!("write cert failed: {}", e)))?;
    tokio::fs::write(&key_path, &req.key_pem)
        .await
        .map_err(|e| ApiError::internal(format!("write key failed: {}", e)))?;

    state.platform.audit.log(
        "cert.update",
        "gateway",
        &format!("upload cert: {}", req.name),
        "api",
    );

    Ok(Json(ApiResponse::ok(())))
}

/// POST /api/certificates/renew — 重新生成自签名证书
pub(crate) async fn renew_certificate(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let mgr = &state.north.cert_manager;
    mgr.generate()
        .map_err(|e| ApiError::internal(format!("generate cert failed: {}", e)))?;

    state.platform.audit.log(
        "cert.renew",
        "gateway",
        "regenerate self-signed cert",
        "api",
    );

    Ok(Json(ApiResponse::ok(())))
}
