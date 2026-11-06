use super::*;

// ===== 纯逻辑：规范化 =====

#[test]
fn test_normalize_id() {
    assert_eq!(normalize_id("  ABCD-1234 \n"), "abcd-1234");
    assert_eq!(normalize_id(""), "");
}

#[test]
fn test_normalize_mac() {
    assert_eq!(normalize_mac("AA:BB:CC:DD:EE:FF"), "aabbccddeeff");
    assert_eq!(normalize_mac("aa-bb-cc-dd-ee-ff"), "aabbccddeeff");
    assert_eq!(normalize_mac(""), "");
}

#[test]
fn test_hash_field_empty_is_none() {
    assert_eq!(hash_id_field("  "), None);
    assert_eq!(hash_mac_field(""), None);
    assert_eq!(hash_id_field("abc").unwrap(), sha256_hex(b"abc"));
}

// ===== 纯逻辑：接口名过滤 =====

#[test]
fn test_interface_filter() {
    // 蜂窝/虚拟必须排除（§6：wwan MAC 变化不影响指纹）
    for excluded in [
        "lo",
        "wwan0",
        "wwx0123456789",
        "usb0",
        "docker0",
        "vethabc123",
        "br-1a2b",
        "virbr0",
        "tun0",
        "tap9",
        "wg0",
    ] {
        assert!(
            is_excluded_interface(excluded),
            "{} should be excluded",
            excluded
        );
    }
    // 板载物理网卡保留
    for kept in ["eth0", "eth1", "end0", "enp1s0", "wlan0"] {
        assert!(!is_excluded_interface(kept), "{} should be kept", kept);
    }
}

// ===== 纯逻辑：分层比对 =====

fn fp(
    key: Option<&str>,
    mac: Option<&str>,
    storage: Option<&str>,
    serial: Option<&str>,
) -> LocalFingerprints {
    LocalFingerprints {
        machine_key: key.map(str::to_string),
        onboard_mac: mac.map(str::to_string),
        storage_serial: storage.map(str::to_string),
        device_serial: serial.map(str::to_string),
    }
}

#[test]
fn test_compare_primary_key_hit() {
    let local = fp(Some("K1"), Some("M"), Some("S"), Some("D"));
    // 主键命中直接通过，回退项全不匹配也无关
    assert!(compare(&local, "K1", &["x".into(), "y".into(), "z".into()]));
}

#[test]
fn test_compare_fallback_dynamic_threshold() {
    // 主键缺失，3 项可比对，匹配 2 → ⌈6/3⌉=2 → 通过
    let local = fp(None, Some("M"), Some("S"), Some("X"));
    assert!(compare(&local, "", &["M".into(), "S".into(), "D".into()]));
    // 匹配 1 → 拒绝
    let local1 = fp(None, Some("M"), Some("X"), Some("X"));
    assert!(!compare(&local1, "", &["M".into(), "S".into(), "D".into()]));
}

#[test]
fn test_compare_device_serial_missing_n2() {
    // 设备序列号缺失的板子（payload[2] 为空串）：n=2，两项全中 → 通过
    let local = fp(None, Some("M"), Some("S"), None);
    assert!(compare(
        &local,
        "",
        &["M".into(), "S".into(), String::new()]
    ));
    // n=2 只中 1 → ⌈4/3⌉=2 → 拒绝
    let local2 = fp(None, Some("M"), Some("X"), None);
    assert!(!compare(
        &local2,
        "",
        &["M".into(), "S".into(), String::new()]
    ));
}

#[test]
fn test_compare_no_comparable_fails_closed() {
    // 主键缺失 + 本机无任何回退项 → 无可比对项 → 拒绝
    let local = fp(None, None, None, None);
    assert!(!compare(&local, "", &["M".into(), "S".into(), "D".into()]));
}

#[test]
fn test_compare_local_missing_item_not_counted() {
    // payload 带序列号哈希但本机采不到 → 该项不可比，n=2
    let local = fp(None, Some("M"), Some("S"), None);
    assert!(compare(&local, "", &["M".into(), "S".into(), "D".into()]));
}

// ===== 采集：伪 sysfs（root 注入，Windows 开发机/CI 均可跑）=====

/// 在 root 下构造伪 sysfs 树（root 由测试体内的 tempdir 提供）
fn fake_sysfs(root: &Path, ifaces: &[(&str, &str)], files: &[(&str, &str)]) {
    for (name, mac) in ifaces {
        let d = root.join("sys/class/net").join(name);
        std::fs::create_dir_all(d.join("device")).unwrap();
        std::fs::write(d.join("address"), mac).unwrap();
    }
    for (rel, content) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
}

#[test]
fn test_collect_linux_full() {
    let dir = tempfile::tempdir().unwrap();
    fake_sysfs(
        dir.path(),
        &[
            ("eth0", "AA:BB:CC:DD:EE:01"),
            ("wwan0", "AA:BB:CC:DD:EE:99"), // 蜂窝（有 device 节点），仍须按名称过滤
            ("lo", "00:00:00:00:00:00"),    // 回环
        ],
        &[
            ("etc/machine-id", "0123456789abcdef0123456789abcdef\n"),
            ("sys/block/mmcblk0/device/cid", "1A2B3C4D\n"),
            ("proc/device-tree/serial-number", "10000000abc\0"),
        ],
    );
    let fp = collect_linux(dir.path());
    assert_eq!(
        fp.machine_key.unwrap(),
        sha256_hex(b"0123456789abcdef0123456789abcdef")
    );
    // MAC 规范化后哈希（小写去分隔符），且取的是 eth0 而非 wwan0
    assert_eq!(fp.onboard_mac.unwrap(), sha256_hex(b"aabbccddee01"));
    assert_eq!(fp.storage_serial.unwrap(), sha256_hex(b"1a2b3c4d"));
    assert_eq!(fp.device_serial.unwrap(), sha256_hex(b"10000000abc"));
}

#[test]
fn test_collect_linux_virtual_nic_without_device_node_skipped() {
    // docker0 无 device 节点（虚拟网卡特征），仅剩 eth0
    let dir = tempfile::tempdir().unwrap();
    let eth = dir.path().join("sys/class/net/eth0");
    std::fs::create_dir_all(eth.join("device")).unwrap();
    std::fs::write(eth.join("address"), "aa:bb:cc:dd:ee:01").unwrap();
    let docker = dir.path().join("sys/class/net/docker0");
    std::fs::create_dir_all(&docker).unwrap();
    std::fs::write(docker.join("address"), "aa:bb:cc:dd:ee:02").unwrap();

    let fp = collect_linux(dir.path());
    assert_eq!(fp.onboard_mac.unwrap(), sha256_hex(b"aabbccddee01"));
}

#[test]
fn test_collect_linux_adaptive_missing() {
    // 全部缺失 → 全 None（自适应，比对时 n 由可比对项决定）
    let dir = tempfile::tempdir().unwrap();
    let fp = collect_linux(dir.path());
    assert_eq!(fp, LocalFingerprints::default());
}

#[test]
fn test_collect_linux_multi_nic_deterministic() {
    // 多网卡取接口名字典序第一个：eth0 < eth1
    let dir = tempfile::tempdir().unwrap();
    fake_sysfs(
        dir.path(),
        &[("eth1", "AA:BB:CC:DD:EE:02"), ("eth0", "AA:BB:CC:DD:EE:01")],
        &[],
    );
    let fp = collect_linux(dir.path());
    assert_eq!(fp.onboard_mac.unwrap(), sha256_hex(b"aabbccddee01"));
}

// ===== 纯逻辑：USB 承载网卡的识别 =====

/// r15-LIC1 (fix): USB-Ethernet dongle 的 sysfs 路径经过 `usb<N>` 控制器目录
/// → 必须被排除，否则插拔外设会导致许可证指纹漂移（现场"授权莫名失效"）。
#[test]
fn test_usb_backed_paths_are_detected() {
    // 典型 USB 网卡：pci → usb1 控制器 → 网卡接口
    assert!(is_usb_backed_path(std::path::Path::new(
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-1/1-1:1.0"
    )));
    // 多控制器时的 usb2 也要命中
    assert!(is_usb_backed_path(std::path::Path::new(
        "/sys/devices/platform/soc/usb2/1-2/net/eth1"
    )));
    // 板载网卡（PCI/SoC 路径，无 usb<N> 段）→ 不判定为 USB
    assert!(!is_usb_backed_path(std::path::Path::new(
        "/sys/devices/pci0000:00/0000:00:14.0/net/eth0"
    )));
    assert!(!is_usb_backed_path(std::path::Path::new(
        "/sys/devices/platform/soc/fe210000/ethernet@fe210000/net/eth0"
    )));
    // 目录名只是「含 usb」但不是 `usb` + 序号 → 不误判
    assert!(!is_usb_backed_path(std::path::Path::new(
        "/sys/devices/platform/soc/usb_controller/net/eth0"
    )));
    assert!(!is_usb_backed_path(std::path::Path::new(
        "/sys/devices/platform/soc/usbnet/net/eth0"
    )));
}

// ===== 整体指纹 =====

#[test]
fn test_compute_overall_shape() {
    let a = compute_overall("K", &["M", "S", "D"]);
    let b = compute_overall("K", &["M", "S", ""]);
    assert_ne!(a, b);
    assert_eq!(a, compute_overall("K", &["M", "S", "D"])); // 确定性
}
