//! Provenance is optional and belongs only to recognized VM-limit data.

use monty_proto::{
    decode_frame, encode_to_capped_vec, pb,
    pb::vm_limit::{Kind, Memory, RecursionDepth, Time},
};
use monty_types::{ExcData, ExcType, MontyException, VmLimit, VmLimitData, VmLimitSource};

/// A known or future source survives the same structured exception wire path.
#[test]
fn vm_limit_source_round_trips_inside_exc_data() {
    let kind = VmLimit::FeedTime {
        limit: 50_000_000,
        elapsed: 51_000_000,
    };
    for (wire_source, source) in [
        (None, None),
        (Some(1), Some(VmLimitSource::HostedPolicy)),
        (Some(72), Some(VmLimitSource::Unknown(72))),
    ] {
        let data = VmLimitData::from(kind).with_source(source);
        let exception = MontyException::new(ExcType::TimeoutError, None).with_data(ExcData::VmLimit(Box::new(data)));
        let wire = pb::RaisedException::from(&exception);
        let Some(pb::exc_data::Kind::VmLimit(limit)) = wire.data.as_ref().and_then(|data| data.kind.as_ref()) else {
            panic!("expected structured VM limit");
        };
        assert_eq!(limit.source, wire_source);
        let frame = encode_to_capped_vec(&wire).unwrap();
        let decoded = decode_frame::<pb::RaisedException>(&frame).unwrap();
        assert_eq!(MontyException::try_from(decoded).unwrap(), exception);
    }
}

/// Unattributed limit kinds retain their measurements even if a sender includes a source.
#[test]
fn turn_and_recursion_limits_have_no_policy_source() {
    for (kind, expected) in [
        (
            Kind::TurnTime(Time {
                limit: Some(5),
                elapsed: Some(6),
            }),
            VmLimit::TurnTime { limit: 5, elapsed: 6 },
        ),
        (
            Kind::RecursionDepth(RecursionDepth {
                limit: Some(5),
                depth: Some(6),
            }),
            VmLimit::RecursionDepth { limit: 5, depth: 6 },
        ),
    ] {
        let data = VmLimitData::from(pb::VmLimit {
            kind: Some(kind),
            source: Some(1),
        });
        assert_eq!(data.kind(), expected);
        assert_eq!(data.source(), None);
    }
}

/// Missing measurements or a missing recognized arm cannot support an attribution.
#[test]
fn unrecognized_limits_do_not_report_a_source() {
    for kind in [
        Some(Kind::Memory(Memory {
            limit: Some(1_000),
            used: None,
        })),
        Some(Kind::FeedTime(Time {
            limit: None,
            elapsed: Some(2_000),
        })),
        None,
    ] {
        let data = VmLimitData::from(pb::VmLimit { kind, source: Some(1) });
        assert_eq!(data.kind(), VmLimit::Unknown);
        assert_eq!(data.source(), None);
    }
}
