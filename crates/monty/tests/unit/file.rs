use monty_types::{PrintWriter, ResourceTracker};

use super::*;
use crate::{
    defer_drop_mut,
    heap::HeapReader,
    intern::CompileInterns,
    run::{Program, SessionTables},
};

#[test]
fn binary_file_reads_interned_bytes_from_buffer() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    let mut tables = SessionTables::default();
    let program = Program::for_tests();
    let id = CompileInterns::direct(&mut tables.interns).intern_bytes(b"\xff\n");
    HeapReader::with(&mut heap, &mut (&program, &mut tables), |reader, (program, tables)| {
        let vm = &mut VM::new(Vec::new(), tables, program, reader, PrintWriter::Disabled);
        let file = vm
            .heap
            .allocate_as(OpenFile::with_state("/data".to_owned(), FileMode::Read(true), 0));
        defer_drop_mut!(file, vm);
        let file = file.read(vm.heap);
        install_buffer(file, Value::InternBytes(id), vm).unwrap();
        let line = next_line(file, vm).unwrap().unwrap();
        defer_drop!(line, vm);
        assert_eq!(
            line.read_as::<Bytes>(vm.heap).unwrap().get(vm.heap).as_slice(),
            b"\xff\n"
        );
        assert!(next_line(file, vm).unwrap().is_none());
    });
}
