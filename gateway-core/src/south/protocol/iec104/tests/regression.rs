//! 修复(C-4) 运行时回归：非法 APDU 长度必须 fail-closed
//!
//! 修复前 `read_frame` 对 length = 0/1/2/3 直接返回 Ok(空或过短 body)：
//! 调用点只能 `continue` 空转（对端持续发 `68 00` 即在 deadline 内忙轮询），
//! STARTDT 握手处更会把垃圾帧当成"对端已应答"、在链路未建立时置 connected=true。
//! 修复后长度域不在 4..=253 时立即返回 InvalidFrame，且不为超长帧分配/阻塞读取。
use super::*;

#[tokio::test]
async fn c4_invalid_apdu_length_is_rejected() {
    // 0..3 过短（控制域不足 4 字节）；254/255 超过 104 单包上限
    for len in [0u8, 1, 3, 254, 255] {
        let err = read_frame_err_with(&[0x68, len]).await;
        assert!(
            matches!(err, crate::south::protocol::CollectorError::InvalidFrame(_)),
            "length={len} 应判为 InvalidFrame，实际 {err:?}"
        );
    }
}

#[tokio::test]
async fn c4_min_valid_apdu_length_still_accepted() {
    // 4 字节控制域（S-frame）是合法的最短 APDU，不能被误判
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口");
    let addr = listener.local_addr().expect("取本地端口");
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        s.write_all(&[0x68, 0x04, 0x01, 0x00, 0x02, 0x00])
            .await
            .expect("write");
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let mut client = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let body = read_frame(&mut client).await.expect("合法最短帧应被接受");
    assert_eq!(body.len(), 4);
    assert_eq!(body[0], 0x01);
    server.abort();
}
