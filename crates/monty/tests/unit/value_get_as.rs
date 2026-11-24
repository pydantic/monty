use monty_types::ResourceTracker;

use crate::{
    heap::Heap,
    intern::Interns,
    types::{Bytes, NamedTuple},
    value::Value,
};

#[test]
fn get_as_borrows_inline_and_boxed_payloads() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    let bytes = Value::allocate(Bytes::new(vec![1, 2]), &heap);
    let point = Value::allocate(NamedTuple::new("Point".to_owned(), vec![], vec![]), &heap);

    assert_eq!(bytes.get_as::<Bytes>(&heap).unwrap().as_slice(), &[1, 2]);
    assert_eq!(
        point.get_as::<NamedTuple>(&heap).unwrap().name(&Interns::default()),
        "Point"
    );
    assert!(point.get_as::<Bytes>(&heap).is_none());
    assert!(Value::None.get_as::<Bytes>(&heap).is_none());

    point.drop_with(&mut heap);
    bytes.drop_with(&mut heap);
}
