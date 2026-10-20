# Heap ownership and pointer safety

`HeapOwner` contains the arena, idle globals and every suspended VM root.
Suspended VM state is boxed so idle owners do not reserve storage for it.
Only its implementation can construct an arena or erase its invariant lifetime.
`activate` and `inspect` expose the complete state through higher-ranked callbacks; their return types cannot contain the fresh brand.
Arena storage cannot be exchanged with another owner's storage while retaining its roots.
Only complete owners implement `Send`; neither owners nor active pointers implement `Sync`.

`HeapId<'arena>` is a non-owning, single-word pointer.
Owning references remain explicit: `Value::Ref` and documented owned ID fields increment and decrement reference counts.
An arena brand proves membership, not object liveness or reference-count correctness.
Slot metadata remains allocated after a payload is destroyed, so numeric identity remains readable during cleanup and slot reuse.
Payload access checks the live flag, and destruction checks the reader count before taking exclusive access.

`HeapReader` creates a second invariant lifetime for scoped reads.
`HeapRead::get` and `HeapReadMut::get_mut` borrow both the capability and that reader, keeping the source alive and excluding conflicting access.
`HeapObjectRead` owns the reader count and exposes its capability only through shared references.
Protected and projected capabilities retain their source borrow; vector and box projections resolve the current allocation on each access.
A scoped `HeapPtr` erases its payload type; its matching reader supplies the arena reference type when dereferencing it.

Each page contains permanent `UnsafeCell<MaybeUninit<HeapEntry>>` slots.
Only the initialized prefix is accessed or destroyed.
Allocation initializes a slot through shared page access: taking `&mut` to an existing page would invalidate other live pointers.
Directory growth may move page boxes, but never their allocations.

Snapshots decode into integer-reference payloads before constructing an owner.
Wire entries omit runtime reader counts, live flags and slot metadata.
Persistence adapters preserve struct boundaries and field order for both map and sequence formats.
Reconstruction reserves every destination slot before converting references, so forward edges, sharing and cycles need no global lookup.
All roots are converted against those same slots; weak indexes are rebuilt afterwards.
Neither runtime pointers nor standalone heaps implement `Deserialize`.
Gray and White GC states are rejected before relocation; other malformed snapshots follow the repository's snapshot trust contract.

Compiler-owned values use `NoHeapRef`, an uninhabited representation that excludes indirect instance-type references too.
`Reference` permits pointer-free graph storage; sealed `RuntimeRef` bounds restrict operations that dereference heap objects.
The owner tests check movement, unwinding, allocation during live reads and slot reuse under Miri.
The compile-fail suite checks cross-owner access, root escape, independent heap construction, deserialization and thread capabilities.
