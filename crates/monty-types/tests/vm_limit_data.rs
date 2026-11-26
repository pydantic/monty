//! Serde retains the limit facts without provenance added for delivery.

use monty_types::{ExcData, VmLimit, VmLimitData, VmLimitSource};
use serde::Serialize;

/// The persisted `ExcData` variant encodes only the bare facts and reloads without a source.
#[test]
fn vm_limit_provenance_is_not_serialized_by_serde() {
    #[derive(Serialize)]
    enum BareExcData {
        VmLimit(Box<VmLimit>),
    }

    let kind = VmLimit::Memory {
        limit: 1_000,
        used: 2_000,
    };
    let data = VmLimitData::from(kind).with_source(Some(VmLimitSource::HostedPolicy));
    let encoded = minicbor_serde::to_vec(ExcData::VmLimit(Box::new(data))).unwrap();
    let bare = minicbor_serde::to_vec(BareExcData::VmLimit(Box::new(kind))).unwrap();
    assert_eq!(encoded, bare);
    let decoded: ExcData = minicbor_serde::from_slice(&encoded).unwrap();
    let ExcData::VmLimit(limit) = decoded else {
        panic!("saved exception lost its VM limit facts");
    };
    assert_eq!(limit.kind(), kind);
    assert_eq!(limit.source(), None);
}
