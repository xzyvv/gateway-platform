//! 三菱 MC 协议级批量读（决策表 #7）
//!
//! 与 `Collector::read_batch` 完全相同的分组合并 / 预构建事务 / 惰性建链 /
//! 顺序下发语义，但每产生一个点位结果就立即写入共享槽 `slots`
//! （await 在锁外、锁内立即写、锁不跨 await）。组超时 drop future 时，
//! 已完成点位的结果仍留在槽中，不会被整批判 Timeout。

use std::collections::BTreeMap;

use crate::config::PointConfig;
use crate::engine::model::{DataPoint, PointDataType};
use crate::south::protocol::{
    clone_collector_error, Collector, CollectorError, CollectorResult, ResultSlots,
};

use super::frames::{
    check_end_code, decode_bit, decode_words, parse_address, plan_batches, McDevice, McFrameFormat,
    MAX_BITS_PER_FRAME, MAX_UNITS_PER_FRAME_1E, MAX_WORDS_PER_FRAME,
};
use super::{build_datapoint, McCollector, McTxn};

/// 批量读合并允许的地址空洞（字元件，单位：字）
const MC_MERGE_GAP_WORDS: u16 = 8;
/// 批量读合并允许的地址空洞（位元件，单位：位）
const MC_MERGE_GAP_BITS: u16 = 16;

impl McCollector {
    /// 缺陷 #7：与 `Collector::read_batch` 完全相同的分组合并 / 预构建事务 / 惰性
    /// 建链 / 顺序下发语义，但每产生一个点位结果就立即写入共享槽 `slots`
    /// （await 在锁外、锁内立即写、锁不跨 await）。组超时 drop future 时，
    /// 已完成点位的结果仍留在槽中，不会被整批判 Timeout。
    pub(crate) async fn read_batch_points_with_slots(
        &self,
        points: &[PointConfig],
        slots: ResultSlots,
    ) -> Vec<CollectorResult<DataPoint>> {
        let n = points.len();
        if n == 0 {
            return Vec::new();
        }
        let s = slots.clone();
        let set = move |idx: usize, r: CollectorResult<DataPoint>| {
            s.lock().unwrap_or_else(|e| e.into_inner())[idx] = Some(r);
        };

        // 点位预解析结果（下标保持为输入顺序，最终按 idx 回填）
        #[derive(Clone)]
        struct Item {
            idx: usize,
            head: u32,
            size: u16,
            is_bit: bool,
            dt: PointDataType,
        }

        // 按（软元件码, 是否位元件）分组：同一组的字元件即使 data_type 不同也可共享一帧
        let mut groups: BTreeMap<(u8, bool), Vec<Item>> = BTreeMap::new();

        for (idx, p) in points.iter().enumerate() {
            let device = match parse_address(&p.address) {
                Ok(d) => d,
                Err(e) => {
                    set(idx, Err(e));
                    continue;
                }
            };
            let dt = p.data_type_enum().unwrap_or(PointDataType::U16);
            let size = if device.is_bit {
                1
            } else {
                dt.register_count()
            };
            groups
                .entry((device.code, device.is_bit))
                .or_default()
                .push(Item {
                    idx,
                    head: device.head,
                    size,
                    is_bit: device.is_bit,
                    dt,
                });
        }

        // 阶段一：预构建全部批量事务。建帧失败（如 1E 帧下 A 系列不可寻址软元件）就地
        // 逐点回填同因错误，不建立连接、不发请求。
        struct Pending {
            txn: McTxn,
            start: u32,
            members: Vec<Item>,
        }
        let mut pending: Vec<Pending> = Vec::new();

        for ((code, is_bit), mut items) in groups {
            items.sort_by_key(|it| it.head);
            let keys: Vec<(u32, u16)> = items.iter().map(|it| (it.head, it.size)).collect();
            let (max_units, gap) = if self.format == McFrameFormat::E1 {
                (
                    MAX_UNITS_PER_FRAME_1E,
                    if is_bit {
                        MC_MERGE_GAP_BITS
                    } else {
                        MC_MERGE_GAP_WORDS
                    },
                )
            } else if is_bit {
                (MAX_BITS_PER_FRAME, MC_MERGE_GAP_BITS)
            } else {
                (MAX_WORDS_PER_FRAME, MC_MERGE_GAP_WORDS)
            };

            for plan in plan_batches(&keys, max_units, gap) {
                let members = &items[plan.range.clone()];
                let device = McDevice {
                    code,
                    head: plan.start,
                    is_bit,
                };
                // 1E 帧格式对部分软元件（A 系列不可寻址）会在建帧时报错 → 该片段逐点回填同因错误
                match self.read_txn(&device, plan.units) {
                    Ok(txn) => pending.push(Pending {
                        txn,
                        start: plan.start,
                        members: members.to_vec(),
                    }),
                    Err(e) => {
                        for it in members {
                            set(it.idx, Err(clone_collector_error(&e)));
                        }
                    }
                }
            }
        }

        // 阶段二：存在待发事务时才建链；无事务时直接返回，不产生无谓连接。
        // 建链失败仍复用整批同因错误语义（整批仅一次尝试，避免逐帧重复等待）。
        if !pending.is_empty() && !self.is_connected() {
            if let Err(e) = self.connect().await {
                let msg = e.to_string();
                for p in &pending {
                    for it in &p.members {
                        set(it.idx, Err(CollectorError::IoError(msg.clone())));
                    }
                }
                // 从槽取回逐点同因错误：with_slots 路径靠槽交付，read_batch 路径靠返回值。
                let mut guard = slots.lock().unwrap_or_else(|e| e.into_inner());
                return guard
                    .iter_mut()
                    .map(|s| {
                        s.take().unwrap_or_else(|| {
                            Err(CollectorError::InvalidFrame(
                                "mc batch internal: point not processed".into(),
                            ))
                        })
                    })
                    .collect();
            }
        }

        for p in &pending {
            match self.transact(&p.txn).await {
                Ok((end_code, data)) => {
                    if let Err(e) = check_end_code(end_code) {
                        let msg = e.to_string();
                        for it in &p.members {
                            set(it.idx, Err(CollectorError::InvalidFrame(msg.clone())));
                        }
                        continue;
                    }
                    for it in &p.members {
                        let offset = (it.head - p.start) as usize;
                        let raw = if it.is_bit {
                            decode_bit(&data, offset)
                        } else {
                            let b = offset * 2;
                            let e = b + it.size as usize * 2;
                            if e <= data.len() {
                                decode_words(&data[b..e], it.dt)
                            } else {
                                None
                            }
                        };
                        set(
                            it.idx,
                            match raw {
                                Some(rv) => Ok(build_datapoint(&points[it.idx], rv)),
                                None => Err(CollectorError::InvalidFrame(format!(
                                    "mc batch decode failed for {} ({} bytes)",
                                    points[it.idx].address,
                                    data.len()
                                ))),
                            },
                        );
                    }
                }
                Err(e) => {
                    // 该片段整体失败：逐点回填同因错误，并保留原始错误变体
                    for it in &p.members {
                        set(it.idx, Err(clone_collector_error(&e)));
                    }
                }
            }
        }

        let mut guard = slots.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter_mut()
            .map(|s| {
                s.take().unwrap_or_else(|| {
                    Err(CollectorError::InvalidFrame(
                        "mc batch internal: point not processed".into(),
                    ))
                })
            })
            .collect()
    }
}
