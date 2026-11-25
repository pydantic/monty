use monty_types::{PrintWriter, ResourceTracker};

use super::*;
use crate::{
    defer_drop_mut,
    heap::HeapReader,
    run::{Program, SessionTables},
};

#[test]
fn tzinfo_from_datetime_dump_without_retained_reference() {
    let datetime: DateTime = serde_json::from_value(serde_json::json!({
        "naive": "2024-01-01T12:00:00",
        "offset_seconds": 5400,
        "timezone_name": "East",
    }))
    .unwrap();
    let mut heap = Heap::new(16, ResourceTracker::default());
    let mut tables = SessionTables::default();
    let program = Program::for_tests();
    HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
        let vm = &mut VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
        let dt = vm.heap.allocate_as(datetime);
        defer_drop_mut!(dt, vm);
        let tz = dt.read(vm.heap).tzinfo_value(vm);
        defer_drop!(tz, vm);
        let tz = tz.read_as::<TimeZone>(vm.heap).unwrap();
        let tz = tz.get(vm.heap);
        assert_eq!(tz.offset_seconds, 5400);
        assert_eq!(tz.name.as_deref(), Some("East"));
    });
}
