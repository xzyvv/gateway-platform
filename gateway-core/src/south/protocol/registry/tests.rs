use super::*;

/// 校验 VALID_PROTOCOLS 与 PROTOCOL_ENTRIES 保持同步
///
/// 防止新增协议时只改一处而漏改另一处：
/// - PROTOCOL_ENTRIES 中所有协议必须在 VALID_PROTOCOLS 中
/// - VALID_PROTOCOLS 中除 gpio 外的协议必须在 PROTOCOL_ENTRIES 中
///   （gpio 是点位级协议，不进注册表）
#[test]
fn test_valid_protocols_match_entries() {
    for entry in PROTOCOL_ENTRIES {
        assert!(
            VALID_PROTOCOLS.contains(&entry.name),
            "PROTOCOL_ENTRIES 协议 '{}' 不在 VALID_PROTOCOLS 中",
            entry.name
        );
    }
    let registered: Vec<&str> = PROTOCOL_ENTRIES.iter().map(|e| e.name).collect();
    for &p in VALID_PROTOCOLS {
        if p == "gpio" {
            continue;
        }
        assert!(
            registered.contains(&p),
            "VALID_PROTOCOLS 协议 '{}' 不在 PROTOCOL_ENTRIES 中",
            p
        );
    }
}

/// 校验 ProtocolRegistry::new() 注册了所有 PROTOCOL_ENTRIES
#[test]
fn test_registry_registers_all_entries() {
    let r = ProtocolRegistry::new();
    for entry in PROTOCOL_ENTRIES {
        assert!(
            r.protocols().contains(&entry.name),
            "协议 '{}' 未被注册",
            entry.name
        );
        if entry.frame_extractor.is_some() {
            assert!(
                r.frame_extractor(entry.name).is_some(),
                "协议 '{}' 的帧提取器未注册",
                entry.name
            );
        } else {
            assert!(
                r.frame_extractor(entry.name).is_none(),
                "协议 '{}' 不应有帧提取器",
                entry.name
            );
        }
    }
}

/// 未注册协议的 frame_extractor 应返回 None
#[test]
fn test_frame_extractor_unknown_returns_none() {
    let r = ProtocolRegistry::new();
    assert!(
        r.frame_extractor("nonexistent_protocol").is_none(),
        "未注册协议的帧提取器应返回 None"
    );
}

/// protocols() 返回非空列表且包含内置协议 modbus
///
/// modbus 是 feature gate 协议：协议裁剪构建（`--no-default-features --features ...`）
/// 里根本不存在，"注册表含 modbus" 这条断言在那些构建下无从成立，故随 feature 门控。
#[cfg(feature = "modbus")]
#[test]
fn test_protocols_returns_builtin() {
    let r = ProtocolRegistry::new();
    let protocols = r.protocols();
    assert!(!protocols.is_empty(), "注册表不应为空");
    assert!(
        protocols.contains(&"modbus"),
        "modbus 应在注册表中，实际: {:?}",
        protocols
    );
}
/// S1: 协议接入一致性自检——强制 VALID_PROTOCOLS / PROTOCOL_ENTRIES / frame_extract 三者同步
///
/// 新增协议时若只改一处而漏改另一处，此测试立即失败。
/// 三维度校验：
/// 1. VALID_PROTOCOLS ⊇ PROTOCOL_ENTRIES 协议名（gpio 除外）
/// 2. PROTOCOL_ENTRIES 中所有协议名在 VALID_PROTOCOLS 中
/// 3. PROTOCOL_ENTRIES.frame_extractor 标记与 ProtocolRegistry 查找结果一致
#[test]
fn test_protocol_three_way_sync() {
    // 维度 1+2: VALID_PROTOCOLS 与 PROTOCOL_ENTRIES 双向覆盖
    for entry in PROTOCOL_ENTRIES {
        assert!(
            VALID_PROTOCOLS.contains(&entry.name),
            "三向同步失败: PROTOCOL_ENTRIES 协议 '{}' 不在 VALID_PROTOCOLS",
            entry.name
        );
    }
    let entry_names: Vec<&str> = PROTOCOL_ENTRIES.iter().map(|e| e.name).collect();
    for &p in VALID_PROTOCOLS {
        if p == "gpio" {
            continue; // gpio 是点位级协议，不进注册表
        }
        assert!(
            entry_names.contains(&p),
            "三向同步失败: VALID_PROTOCOLS 协议 '{}' 不在 PROTOCOL_ENTRIES",
            p
        );
    }

    // 维度 3: frame_extractor 标记与 ProtocolRegistry 查找结果一致
    let registry = ProtocolRegistry::new();
    for entry in PROTOCOL_ENTRIES {
        match entry.frame_extractor {
            Some(_) => assert!(
                registry.frame_extractor(entry.name).is_some(),
                "三向同步失败: '{}' 标记有帧提取器但 registry 查不到",
                entry.name
            ),
            None => assert!(
                registry.frame_extractor(entry.name).is_none(),
                "三向同步失败: '{}' 标记无帧提取器但 registry 查到了",
                entry.name
            ),
        }
    }

    // 额外校验: PROTOCOL_ENTRIES 无重复协议名
    let mut seen = std::collections::HashSet::new();
    for entry in PROTOCOL_ENTRIES {
        assert!(
            seen.insert(entry.name),
            "三向同步失败: PROTOCOL_ENTRIES 中协议名 '{}' 重复",
            entry.name
        );
    }

    // 额外校验: VALID_PROTOCOLS 无重复
    let mut seen = std::collections::HashSet::new();
    for &p in VALID_PROTOCOLS {
        assert!(
            seen.insert(p),
            "三向同步失败: VALID_PROTOCOLS 中 '{}' 重复",
            p
        );
    }
}

/// 校验 gpio 能力元数据：点位级协议不进 PROTOCOL_ENTRIES，但应出现在
/// `all_protocol_capabilities()` 且声明 supports_write=true，
/// 以便 Web `GET /api/protocols` 正确门控 GPIO 写入入口。
#[test]
fn test_gpio_capability_present_and_writable() {
    assert!(!PROTOCOL_ENTRIES.iter().any(|e| e.name == "gpio"));
    assert!(VALID_PROTOCOLS.contains(&"gpio"));

    let caps = all_protocol_capabilities();
    let gpio = caps
        .iter()
        .find(|(name, _)| *name == "gpio")
        .map(|(_, c)| *c)
        .expect("all_protocol_capabilities 应包含 gpio");
    assert!(gpio.supports_write, "gpio 应声明 supports_write=true");
    assert!(!gpio.supports_frame_extract, "gpio 无需帧提取器");
    assert!(!gpio.supports_batch_read, "gpio 无批量读");
    assert!(!gpio.supports_scan_slaves, "gpio 无扫描从站");
}

/// P5：标签色由注册表单一来源下发，前端不再按协议名硬编码配色。
///
/// 反证：若某个 `PROTOCOL_ENTRIES` 项或 gpio 描述符填了非法 tag_color，
/// 前端 `el-tag` 会静默回退到默认色（视觉不一致且无报错），故在此锁定白名单。
#[test]
fn test_protocol_tag_color_is_valid() {
    const VALID: &[&str] = &["primary", "success", "warning", "info", "danger"];
    for entry in PROTOCOL_ENTRIES {
        assert!(
            VALID.contains(&entry.tag_color),
            "协议 '{}' 的 tag_color '{}' 非法，应为 {:?} 之一",
            entry.name,
            entry.tag_color,
            VALID
        );
    }
    for d in all_protocol_descriptors() {
        assert!(
            VALID.contains(&d.tag_color),
            "描述符 '{}' 的 tag_color '{}' 非法",
            d.name,
            d.tag_color
        );
    }
}

/// P5：每个协议都必须填标签色（新增协议漏填会被此处发现，而非前端静默回退）。
#[test]
fn test_protocol_tag_color_not_empty() {
    for entry in PROTOCOL_ENTRIES {
        assert!(
            !entry.tag_color.trim().is_empty(),
            "协议 '{}' 未填 tag_color，前端将回落到默认色",
            entry.name
        );
    }
}

/// 回归钉子：能力位必须与实现一致，否则前端按钮可点但后端必然拒绝。
///
/// 从站扫描是协议工具的独立实现（`api::protocol_tools::handlers::slave_scan`
/// 对非 `modbus` 总线返回 `UnsupportedProtocol`），`Collector` 上并没有扫描方法。
/// BACnet 两个变体因此必须声明 `supports_scan_slaves = false`。
#[test]
fn test_scan_slaves_only_modbus() {
    for entry in PROTOCOL_ENTRIES {
        if entry.name == "bacnet" || entry.name == "bacnet_mstp" {
            assert!(
                !entry.capabilities.supports_scan_slaves,
                "协议 '{}' 未实现 scan_slaves，能力位必须为 false（否则前端显示可扫描但请求必失败）",
                entry.name
            );
        }
    }
}

/// 链路为 TCP 的协议必须给出 `tcp_addr` 填写提示，否则 Web 表单只能凭猜测填端点。
///
/// Modbus 在注册表里是 serial，但支持填 `tcp_addr` 切到 Modbus TCP，同样必须有提示。
#[test]
fn test_tcp_protocols_have_endpoint_hint() {
    for entry in PROTOCOL_ENTRIES {
        if entry.transport == "tcp" || entry.name == "modbus" {
            assert!(
                !entry.endpoint_hint.trim().is_empty(),
                "协议 '{}' 可走 TCP，但未填 endpoint_hint（Web 表单将无端点填写提示）",
                entry.name
            );
        }
    }
}

/// 有变体概念的协议必须给出可选值，且不能只声明不实现。
#[test]
fn test_variant_protocols_declare_options() {
    for entry in PROTOCOL_ENTRIES {
        let should_have_variants =
            matches!(entry.name, "modbus" | "dlt645" | "cjt188" | "fins" | "mc");
        assert_eq!(
            should_have_variants,
            !entry.variants.is_empty(),
            "协议 '{}' 的 variants 声明与实际是否有变体概念不一致",
            entry.name
        );
        for (value, label) in entry.variants {
            assert!(
                !value.trim().is_empty() && !label.trim().is_empty(),
                "协议 '{}' 存在空的 variant 取值/展示名",
                entry.name
            );
        }
    }
}

/// N-8 回归钉子：`supports_diag` 必须与「是否覆写 `diag`」一致。
///
/// 假 "ok" 会让运维把「未实现诊断」读成「设备正常」，故未覆写 `diag` 的协议
/// 一律声明 false；当前实现：s7comm（SZL）、bacnet（Who-Is）、bacnet_mstp（Who-Is 广播探针）、
/// iec104（STARTDT + 总召唤）、gpio（芯片信息）、opcua（ServerStatus）、
/// iec101（链路复位 + 总召唤）、dlt645/cjt188（广播读通信地址）、mc/fins（读一个字元件）、
/// modbus（FC03 实读）。
#[test]
fn test_supports_diag_only_for_implemented_protocols() {
    const DIAG_PROTOCOLS: &[&str] = &[
        "s7comm",
        "bacnet",
        "bacnet_mstp",
        "iec104",
        "gpio",
        "opcua",
        "iec101",
        "dlt645",
        "cjt188",
        "mc",
        "fins",
        "modbus",
    ];
    for entry in PROTOCOL_ENTRIES {
        let expected = DIAG_PROTOCOLS.contains(&entry.name);
        assert_eq!(
            entry.capabilities.supports_diag, expected,
            "协议 '{}' 的 supports_diag 与 diag 实现不一致（期望 {}）",
            entry.name, expected
        );
    }
    let gpio = all_protocol_capabilities()
        .into_iter()
        .find(|(name, _)| *name == "gpio")
        .map(|(_, c)| c)
        .expect("all_protocol_capabilities 应包含 gpio");
    assert!(gpio.supports_diag, "gpio 覆写了 diag，能力位必须为 true");
}

/// B3 回归钉子：调试工具按能力位路由，不再按协议名硬编码。
///
/// 新增协议只要在注册表声明 `supports_scan_slaves` /
/// `supports_register_scan` / `supports_auto_detect`，工具端即可用；
/// 未声明或协议未注册时由调用方给出区别化提示。
///
/// 断言按协议 feature 门控：协议裁剪构建（`--no-default-features`）与
/// 「退役观察」协议（fins / bacnet-mstp 已移出默认发行集）下该协议根本未注册，
/// 能力位无从谈起，故不参与本钉子。
#[test]
fn test_debug_scan_capabilities_are_registry_driven() {
    #[cfg(feature = "modbus")]
    {
        let modbus = protocol_capabilities("modbus").expect("modbus 应已注册");
        assert!(modbus.supports_scan_slaves);
        assert!(modbus.supports_register_scan);
        assert!(modbus.supports_auto_detect);
    }

    // mc / fins 声明 supports_register_scan 后，handler 才按软元件地址逐点探测；
    // 能力位与 handler 实现集合必须一致，否则前端显示入口但请求必被拒。
    #[cfg(feature = "mc")]
    {
        let caps = protocol_capabilities("mc").expect("mc 应已注册");
        assert!(
            caps.supports_register_scan,
            "协议 'mc' 已实现按软元件地址探测，必须声明 supports_register_scan"
        );
        assert!(
            !caps.supports_scan_slaves,
            "协议 'mc' 未实现从站扫描，不得声明支持"
        );
    }
    #[cfg(feature = "fins")]
    {
        let caps = protocol_capabilities("fins").expect("fins 应已注册");
        assert!(
            caps.supports_register_scan,
            "协议 'fins' 已实现按软元件地址探测，必须声明 supports_register_scan"
        );
        assert!(
            !caps.supports_scan_slaves,
            "协议 'fins' 未实现从站扫描，不得声明支持"
        );
    }

    #[cfg(feature = "dlt645")]
    {
        let dlt = protocol_capabilities("dlt645").expect("dlt645 应已注册");
        assert!(
            !dlt.supports_scan_slaves,
            "dlt645 未实现从站扫描，不得声明支持"
        );
        assert!(!dlt.supports_register_scan);
        assert!(!dlt.supports_auto_detect);
        assert!(dlt.supports_diag, "dlt645 已实现诊断（读通信地址）");
    }

    #[cfg(feature = "bacnet-mstp")]
    {
        let mstp = protocol_capabilities("bacnet_mstp").expect("bacnet_mstp 应已注册");
        assert!(mstp.supports_diag, "bacnet_mstp 已实现 Who-Is 广播探针诊断");
    }

    assert!(protocol_capabilities("no-such-protocol").is_none());
}

/// 回归钉子：`supports_write` 必须与「是否覆写 `fn write`」一致。
///
/// # 为什么必须有此钉子
/// 能力位与实现脱节会造成两类现场故障，且**都不可自愈**：
/// - **撒谎位**（声明 true 但未实现 `write`）→ 前端显示写入入口，用户点击后
///   必然拿到 `CollectorError::Unsupported`，白跑一趟且误以为设备故障。
///   历史 Bug 175（opcua）即此类，当时只修了个案、未建防复发机制。
/// - **漏报位**（已实现 `write` 但能力位为 false）→ 写入功能实际可用却被前端
///   隐藏，用户看不到入口，误判网关不支持写。
///
/// 本钉子以「已实现 `fn write` 的协议集合」为唯一判据（白名单），
/// 新增协议若覆写了 `write` 却未同步此集合，测试立即失败并指出该协议名。
///
/// 覆写 `fn write` 的协议（逐文件核对）：
/// modbus / dlt645 / cjt188 / iec104 / mc / fins / bacnet / s7comm。
/// gpio 是点位级协议、不在 `PROTOCOL_ENTRIES` 中，单独断言。
#[test]
fn test_supports_write_matches_implementation() {
    const WRITE_PROTOCOLS: &[&str] = &[
        "modbus", "dlt645", "cjt188", "iec104", "mc", "fins", "bacnet", "s7comm",
    ];
    for entry in PROTOCOL_ENTRIES {
        let expected = WRITE_PROTOCOLS.contains(&entry.name);
        assert_eq!(
            entry.capabilities.supports_write, expected,
            "协议 '{}' 的 supports_write 与 write 实现不一致（期望 {}）",
            entry.name, expected
        );
    }
    // gpio：覆写了 `fn write`（gpio/mod.rs），能力位必须为 true
    let gpio = all_protocol_capabilities()
        .into_iter()
        .find(|(name, _)| *name == "gpio")
        .map(|(_, c)| c)
        .expect("all_protocol_capabilities 应包含 gpio");
    assert!(gpio.supports_write, "gpio 覆写了 write，能力位必须为 true");
}

/// 回归钉子：`supports_batch_read` 必须与「`read_batch` 是否带来超出逐点降化的
/// 实际收益」一致。
///
/// # 判据说明（为何不是简单的「覆写了 read_batch」）
/// - **真正的批量读**：modbus（多寄存器一次帧）、opcua（多节点一次请求）、
///   iec104 / iec101（一次总召唤刷全站）、mc（按软元件分组合并帧）、
///   bacnet（ReadPropertyMultiple）、s7comm（按协商 PDU 分块）、
///   fins（整批共享一次建链，避免 N 倍连接超时）。
/// - **逐点退化，声明 false 属实**：dlt645 / cjt188 虽覆写了 `read_batch`，
///   但其实现被刻意改回「每点一个标准单 DI 事务」（见各自 Bug 223 / 224 注释：
///   多 DI 拼接属厂家方言非标扩展，响应切分靠帧内搜 DI 字节模式，
///   数据值恰好命中即误切分 → 静默数据错误）。二者与 trait 默认实现语义等价，
///   故能力位声明 `false`——前端徽标显示「单点读」是**如实**的。
///
/// 与 `test_supports_write_matches_implementation` 同理：本集合是
/// 「具备批量读收益的协议」的唯一判据，能力位与它不一致即失败。
#[test]
fn test_supports_batch_read_matches_implementation() {
    const BATCH_READ_PROTOCOLS: &[&str] = &[
        "modbus", "opcua", "iec104", "iec101", "mc", "fins", "bacnet", "s7comm",
    ];
    for entry in PROTOCOL_ENTRIES {
        let expected = BATCH_READ_PROTOCOLS.contains(&entry.name);
        assert_eq!(
            entry.capabilities.supports_batch_read, expected,
            "协议 '{}' 的 supports_batch_read 与 read_batch 实际收益不一致（期望 {}）",
            entry.name, expected
        );
    }
    // gpio：无批量读收益
    let gpio = all_protocol_capabilities()
        .into_iter()
        .find(|(name, _)| *name == "gpio")
        .map(|(_, c)| c)
        .expect("all_protocol_capabilities 应包含 gpio");
    assert!(
        !gpio.supports_batch_read,
        "gpio 无批量读收益，能力位必须为 false"
    );
}

/// N-8 回归钉子：`supports_write_multi` 只在真正覆写 `write_multi`
/// （单事务合并写多点）的协议上为 true，避免前端以为能批量写而实际逐点退化。
#[test]
fn test_supports_write_multi_only_native() {
    const NATIVE_WRITE_MULTI: &[&str] = &["modbus", "s7comm"];
    for entry in PROTOCOL_ENTRIES {
        let expected = NATIVE_WRITE_MULTI.contains(&entry.name);
        assert_eq!(
            entry.capabilities.supports_write_multi, expected,
            "协议 '{}' 的 supports_write_multi 与 write_multi 实现不一致（期望 {}）",
            entry.name, expected
        );
    }
    assert!(
        !all_protocol_capabilities()
            .into_iter()
            .find(|(name, _)| *name == "gpio")
            .map(|(_, c)| c)
            .expect("gpio")
            .supports_write_multi,
        "gpio 未覆写 write_multi，能力位必须为 false"
    );
}
