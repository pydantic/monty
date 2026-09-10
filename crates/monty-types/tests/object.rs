//! Tests for `MontyObject`'s host-side footprint: the enum's size (which every
//! container element pays) and the `host_size` accounting of boxed payloads.

use std::mem::size_of;

use monty_types::{DictPairs, MontyClassInstance, MontyClassType, MontyObject, MontyType, MontyUuid};

/// Every `Vec<MontyObject>` element costs this much: the widest inline
/// variant is `NamedTuple` (three 24-byte fields), so a larger payload must
/// be boxed rather than inlined (see `ClassInstance`).
#[test]
fn monty_object_is_72_bytes() {
    assert_eq!(size_of::<MontyObject>(), 72);
    assert_eq!(MontyObject::host_base_size(), 72);
}

fn class_type(attrs: DictPairs) -> MontyClassType {
    MontyClassType {
        name: "Point".to_owned(),
        id: MontyUuid::from_u128(1),
        host_defined: true,
        is_dataclass: false,
        attrs,
    }
}

// === host_size charges the boxed payloads ===

#[test]
fn boxed_payloads_charge_their_allocation() {
    let class_object = MontyObject::Type(MontyType::Instance(Box::new(class_type(DictPairs::default()))));
    assert_eq!(
        class_object.host_size(),
        size_of::<MontyObject>() + size_of::<MontyClassType>() + "Point".len()
    );

    let instance = MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: class_type(DictPairs::default()),
        instance_id: MontyUuid::from_u128(2),
        attrs: DictPairs::default(),
    }));
    assert_eq!(
        instance.host_size(),
        size_of::<MontyObject>() + size_of::<MontyClassInstance>() + "Point".len()
    );
}

// === deep_host_size includes instance attrs and eager class attrs ===

#[test]
fn deep_host_size_sums_instance_and_class_attrs() {
    let pair = |name: &str| (MontyObject::String(name.to_owned()), MontyObject::Int(1));
    let class_attrs = DictPairs::from(vec![pair("ORIGIN")]);
    let instance_attrs = DictPairs::from(vec![pair("x"), pair("y")]);
    let instance = MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: class_type(class_attrs.clone()),
        instance_id: MontyUuid::from_u128(2),
        attrs: instance_attrs.clone(),
    }));
    let pairs_size = |pairs: &DictPairs| {
        pairs
            .iter()
            .map(|(key, value)| key.deep_host_size() + value.deep_host_size())
            .sum::<usize>()
    };
    assert_eq!(
        instance.deep_host_size(),
        instance.host_size() + pairs_size(&instance_attrs) + pairs_size(&class_attrs)
    );
}
