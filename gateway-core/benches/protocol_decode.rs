//! 协议帧解码吞吐量基准测试
//!
//! 运行：`cargo bench --bench protocol_decode`
//! 输出：criterion 统计报告 + target/criterion/html/index.html

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use gateway_core::engine::util::crc::crc16;

/// 构造 Modbus RTU 请求帧（FC03 读保持寄存器）
fn make_modbus_request(slave: u8, start: u16, count: u16) -> Vec<u8> {
    let mut buf = vec![slave, 0x03];
    buf.extend_from_slice(&start.to_be_bytes());
    buf.extend_from_slice(&count.to_be_bytes());
    let crc = crc16(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());
    buf
}

/// 构造 Modbus RTU 响应帧（FC03，N 个寄存器）
fn make_modbus_response(slave: u8, values: &[u16]) -> Vec<u8> {
    let byte_count = (values.len() * 2) as u8;
    let mut buf = vec![slave, 0x03, byte_count];
    for v in values {
        buf.extend_from_slice(&v.to_be_bytes());
    }
    let crc = crc16(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());
    buf
}

fn bench_crc16(c: &mut Criterion) {
    let mut group = c.benchmark_group("crc16");
    for size in [8, 64, 256, 1024] {
        let data = vec![0x42u8; size];
        group.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, d| {
            b.iter(|| black_box(crc16(d)));
        });
    }
    group.finish();
}

fn bench_modbus_frame_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("modbus_frame_build");
    for n in [1, 8, 32, 128] {
        let values: Vec<u16> = (0..n).collect();
        group.bench_with_input(BenchmarkId::from_parameter(n), &values, |b, v| {
            b.iter(|| black_box(make_modbus_response(1, v)));
        });
    }
    group.finish();
}

fn bench_modbus_frame_verify(c: &mut Criterion) {
    // 预生成 N 条帧，基准测试只测 CRC 校验 + 长度检查
    let frames: Vec<Vec<u8>> = (0..256).map(|i| make_modbus_request(1, i, 1)).collect();
    c.bench_function("modbus_verify_256_frames", |b| {
        b.iter(|| {
            for f in &frames {
                if f.len() < 4 {
                    continue;
                }
                let payload = &f[..f.len() - 2];
                let _ = black_box(crc16(payload));
            }
        });
    });
}

criterion_group!(
    benches,
    bench_crc16,
    bench_modbus_frame_build,
    bench_modbus_frame_verify,
);
criterion_main!(benches);
