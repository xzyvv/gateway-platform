//! 点位归属索引：把「按设备反查点位」从 O(设备数 × 点位数) 降到 O(点位数 + 命中数)
//!
//! 设备 → 点位的归属有两条来源：
//! - 显式 `device_id`：点位配置里直接写了所属设备 id；
//! - 设备名前缀：模板展开点位没有 `device_id`，其 id 形如 `{设备名}_{参数名}`，
//!   按 `"{设备名}_"` 前缀匹配（与 topology / health / shadows 三处历史口径一致）。
//!
//! 历史实现对每台设备都线性扫描一遍全部点位（O(D×P)），点位规模上千时成为
//! 热点。此模块一次性建好两张索引（device_id → 点位下标、设备名 → 点位下标），
//! 查询时只做 O(1) 哈希命中；下标列表天然升序，归并即可还原历史的
//! 「按点位顺序输出」语义。

use std::collections::HashMap;

use gateway_core::config::{DeviceConfig, PointConfig};

/// 点位归属索引
pub(crate) struct PointOwnerIndex {
    /// 全局点位 id（与下标一一对应）
    ids: Vec<String>,
    /// device_id → 点位下标（升序）
    by_device_id: HashMap<String, Vec<usize>>,
    /// 设备名 → 点位下标（升序）
    by_device_name: HashMap<String, Vec<usize>>,
    /// 每个点位的归属设备 id（显式 device_id 优先，否则按设备顺序取首个前缀命中）
    owner_id: Vec<Option<String>>,
}

impl PointOwnerIndex {
    /// 用配置点位（显式点位 + 模板展开点位）建索引
    pub(crate) fn from_config_points(
        cfg_points: &[PointConfig],
        expanded: &[PointConfig],
        devices: &[DeviceConfig],
    ) -> Self {
        let items: Vec<(String, Option<String>)> = cfg_points
            .iter()
            .chain(expanded.iter())
            .map(|p| (p.id.clone(), p.device_id.clone().filter(|d| !d.is_empty())))
            .collect();
        Self::build(items, devices)
    }

    /// 通用构造：items 为 (点位 id, 显式 device_id) 序列
    ///
    /// `devices` 提供「设备名 → 设备」的映射与设备顺序（顺序决定多个设备名
    /// 同时前缀命中时取哪一个，与历史 `iter().find()` 行为一致）。
    pub(crate) fn build(items: Vec<(String, Option<String>)>, devices: &[DeviceConfig]) -> Self {
        let ids: Vec<String> = items.iter().map(|(id, _)| id.clone()).collect();

        // 设备名 → (在 devices 中的顺序, 设备 id)
        // 同名设备保留首次出现者：与历史 `devices.iter().find(..)` 口径一致
        let mut name_to_dev: HashMap<&str, (usize, &str)> = HashMap::new();
        for (order, d) in devices.iter().enumerate() {
            if !d.name.is_empty() {
                name_to_dev
                    .entry(d.name.as_str())
                    .or_insert((order, d.id.as_str()));
            }
        }

        let mut by_device_id: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_device_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut owner_id: Vec<Option<String>> = Vec::with_capacity(items.len());

        for (idx, (point_id, explicit_dev)) in items.iter().enumerate() {
            if let Some(dev_id) = explicit_dev {
                by_device_id.entry(dev_id.clone()).or_default().push(idx);
            }
            // 前缀候选：设备名必然是点位 id 在某个 '_' 之前的整段
            // （前缀为 "{设备名}_"，故设备名以 '_' 边界结束）
            let mut matched: Vec<(&str, usize, &str)> = Vec::new();
            for (k, ch) in point_id.char_indices() {
                if ch != '_' || k == 0 {
                    continue;
                }
                let candidate = &point_id[..k];
                if let Some(&(order, dev_id)) = name_to_dev.get(candidate) {
                    matched.push((candidate, order, dev_id));
                }
            }
            for (name, _, _) in &matched {
                by_device_name
                    .entry((*name).to_string())
                    .or_default()
                    .push(idx);
            }
            // 归属设备：显式 device_id 优先；否则取 devices 顺序最靠前的前缀命中
            let owner = explicit_dev.clone().or_else(|| {
                matched
                    .iter()
                    .min_by_key(|(_, order, _)| *order)
                    .map(|(_, _, dev_id)| (*dev_id).to_string())
            });
            owner_id.push(owner);
        }

        Self {
            ids,
            by_device_id,
            by_device_name,
            owner_id,
        }
    }

    /// 某设备的全部点位 id：显式 device_id 命中 + 设备名前缀命中，按全局顺序去重
    pub(crate) fn point_ids_of(&self, device: &DeviceConfig) -> Vec<String> {
        let empty: &[usize] = &[];
        let a = if device.id.is_empty() {
            empty
        } else {
            self.by_device_id
                .get(&device.id)
                .map_or(empty, |v| v.as_slice())
        };
        let b = if device.name.is_empty() {
            empty
        } else {
            self.by_device_name
                .get(&device.name)
                .map_or(empty, |v| v.as_slice())
        };
        merge_indexed(a, b, &self.ids)
    }

    /// 点位的归属设备 id（显式 device_id 优先，其次设备名前缀）
    ///
    /// 按**下标**而非 id 查：同名点位（显式点位与模板展开点位重名）各自按自身
    /// 的 `device_id` 解析，与历史逐点调用 `point_owner_id` 的口径一致。
    pub(crate) fn owner_at(&self, index: usize) -> Option<&str> {
        self.owner_id.get(index).and_then(|o| o.as_deref())
    }
}

/// 归并两个升序下标列表，按全局顺序输出点位 id（同一 id 只出一次）
fn merge_indexed(a: &[usize], b: &[usize], ids: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() || j < b.len() {
        let idx = match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) => {
                if x < y {
                    i += 1;
                    *x
                } else {
                    // x == y：两个来源同时命中该点位，只出一次
                    if x == y {
                        i += 1;
                        j += 1;
                    } else {
                        j += 1;
                    }
                    *y
                }
            }
            (Some(x), None) => {
                i += 1;
                *x
            }
            (None, Some(y)) => {
                j += 1;
                *y
            }
            (None, None) => break,
        };
        out.push(ids[idx].clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 参照实现：优化前的 O(D×P) 线性扫描口径（本文件的历史版本）
    fn legacy_point_ids(
        device: &DeviceConfig,
        cfg_points: &[PointConfig],
        expanded: &[PointConfig],
    ) -> Vec<String> {
        let prefix = format!("{}_", device.name);
        let mut ids = Vec::new();
        for p in cfg_points.iter().chain(expanded.iter()) {
            let by_id = !device.id.is_empty() && p.device_id.as_deref() == Some(device.id.as_str());
            let by_prefix = !device.name.is_empty() && p.id.starts_with(&prefix);
            if by_id || by_prefix {
                ids.push(p.id.clone());
            }
        }
        ids
    }

    /// 参照实现：优化前的逐点归属反查
    fn legacy_owner_id(p: &PointConfig, devices: &[DeviceConfig]) -> Option<String> {
        if let Some(did) = &p.device_id {
            if !did.is_empty() {
                return Some(did.clone());
            }
        }
        devices
            .iter()
            .find(|d| !d.name.is_empty() && p.id.starts_with(&format!("{}_", d.name)))
            .map(|d| d.id.clone())
    }

    fn dev(id: &str, name: &str) -> DeviceConfig {
        DeviceConfig {
            id: id.to_string(),
            name: name.to_string(),
            template: "t".to_string(),
            bus: "COM1".to_string(),
            protocol: "modbus_rtu".to_string(),
            address: "1".to_string(),
            slave_id: Some(1),
            address_map: None,
            interval: None,
        }
    }

    fn pt(id: &str, device_id: Option<&str>) -> PointConfig {
        PointConfig {
            id: id.to_string(),
            name: None,
            device_id: device_id.map(str::to_string),
            bus: "COM1".to_string(),
            protocol: "modbus_rtu".to_string(),
            address: "0".to_string(),
            slave_id: Some(1),
            address_num: Some(0),
            data_id: None,
            function_code: None,
            data_type: None,
            byte_order: None,
            register_count: None,
            interval: 5,
            scale: 1.0,
            offset: 0.0,
            formula: None,
            ct_ratio: None,
            pt_ratio: None,
            deadband: 0.0,
            on_change: false,
            unit: None,
            alarm_high: None,
            alarm_low: None,
            aggregation: None,
            modbus_register: None,
            writable: false,
        }
    }

    #[test]
    fn index_matches_legacy_point_ids() {
        let devices = vec![
            dev("d1", "boiler"),
            dev("d2", "pump"),
            dev("d3", ""), // 空名设备：历史口径下不参与前缀匹配
        ];
        let cfg_points = vec![
            pt("d1_temp", Some("d1")),
            pt("d3_x", Some("d3")),
            pt("loose", None),
        ];
        let expanded = vec![
            pt("boiler_temp", None),
            pt("boiler_press", None),
            pt("pump_flow", None),
        ];
        let index = PointOwnerIndex::from_config_points(&cfg_points, &expanded, &devices);

        for d in &devices {
            assert_eq!(
                index.point_ids_of(d),
                legacy_point_ids(d, &cfg_points, &expanded),
                "device {} 点位集合不一致",
                d.id
            );
        }
    }

    #[test]
    fn index_matches_legacy_owner_id() {
        let devices = vec![dev("d1", "boiler"), dev("d2", "pump")];
        let cfg_points = vec![pt("d1_temp", Some("d1"))];
        let expanded = vec![pt("boiler_temp", None), pt("pump_flow", None)];
        let index = PointOwnerIndex::from_config_points(&cfg_points, &expanded, &devices);

        for (i, p) in cfg_points.iter().chain(expanded.iter()).enumerate() {
            assert_eq!(
                index.owner_at(i).map(str::to_string),
                legacy_owner_id(p, &devices),
                "point {} 归属设备不一致",
                p.id
            );
        }
    }

    #[test]
    fn duplicate_point_id_resolved_per_occurrence() {
        // 显式点位与模板展开点位同名时，各自按自身 device_id 解析
        let devices = vec![dev("d1", "boiler")];
        let cfg_points = vec![pt("boiler_temp", Some("d1"))];
        let expanded = vec![pt("boiler_temp", None)];
        let index = PointOwnerIndex::from_config_points(&cfg_points, &expanded, &devices);

        assert_eq!(index.owner_at(0), Some("d1"));
        assert_eq!(index.owner_at(1), Some("d1"));
    }
}
