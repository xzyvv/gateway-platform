//! BACnet APDU 构建与解析（按用途拆分：decode / write / discovery）
use super::*;

mod decode;
mod discovery;
mod write;

pub(crate) use decode::{apdu_invoke_id, parse_value_from_apdu};
pub(crate) use discovery::{
    build_subscribe_cov_apdu, build_who_is_apdu, parse_cov_notification_apdu, parse_i_am_apdu,
    parse_subscribe_cov_ack, CovNotification, IAmInfo,
};
pub(crate) use write::{
    build_read_property, build_rpm_request_apdu, build_write_property_apdu, parse_rpm_ack,
    parse_write_property_ack, wrap_bvlc_npdu,
};

#[cfg(test)]
pub(crate) use decode::parse_app_value;
#[cfg(test)]
pub(crate) use discovery::dedup_i_am;
