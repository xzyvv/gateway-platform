//! License API 处理函数（授权状态查询 + 授权文件导入）

use super::*;

/// GET /api/license
pub(crate) async fn get_license(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mgr = &state.platform.license;
    let s = mgr.current();
    Json(LicenseSnapshot::to_json_value(&s, mgr.machine()))
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
            Json(LicenseSnapshot::to_json_value(&s, mgr.machine())).into_response()
        }
        Err(e) => import_error(&e).into_response(),
    }
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
