//! Exact cumulative decode charges for the worker's canonical event encoding.

use std::fmt::{self, Display, Write};

use monty_types::unstable::{ClassTypeNode, MontyNode, NodeId};
use prost::encoding::encoded_len_varint;

use crate::{DEFAULT_MAX_DECODE_BYTES, WireArena, pb, wire::REFERENCE_COST};

pub(crate) const ALLOCATION_MODEL_VERSION: u32 = 1;
#[cfg(feature = "worker")]
pub(crate) const DECODE_BUDGET_ERROR: &str = "response exceeds host decode budget";
#[cfg(feature = "worker")]
pub(crate) const REPLACEMENT_ERROR_BYTES: u64 = 47;

/// Builds an offer from this receiver's actual decoder layout and effective ceiling.
#[doc(hidden)]
#[must_use]
pub fn receiver_allocation_profile(max_decode_bytes: usize) -> pb::ReceiverAllocationProfile {
    pb::ReceiverAllocationProfile {
        model_version: ALLOCATION_MODEL_VERSION,
        max_decode_bytes: max_decode_bytes.min(DEFAULT_MAX_DECODE_BYTES) as u64,
        node_slot_bytes: u32::try_from(size_of::<MontyNode>()).expect("node slot fits uint32"),
        reference_slot_bytes: u32::try_from(REFERENCE_COST).expect("reference slot fits uint32"),
        class_box_bytes: u32::try_from(size_of::<ClassTypeNode>()).expect("class box fits uint32"),
        string_slot_bytes: u32::try_from(size_of::<String>()).expect("string slot fits uint32"),
        print_segment_slot_bytes: u32::try_from(size_of::<pb::PrintSegment>()).expect("print segment slot fits uint32"),
        traceback_slot_bytes: u32::try_from(size_of::<pb::StackFrame>()).expect("traceback slot fits uint32"),
    }
}

/// Validates an offer without substituting this sender's layout or budget.
#[cfg(feature = "worker")]
pub(crate) fn recognized_profile(profile: &pb::ReceiverAllocationProfile) -> bool {
    profile.model_version == ALLOCATION_MODEL_VERSION
        && profile.max_decode_bytes >= REPLACEMENT_ERROR_BYTES
        && profile.max_decode_bytes <= DEFAULT_MAX_DECODE_BYTES as u64
        && [
            profile.node_slot_bytes,
            profile.reference_slot_bytes,
            profile.class_box_bytes,
            profile.string_slot_bytes,
            profile.print_segment_slot_bytes,
            profile.traceback_slot_bytes,
        ]
        .into_iter()
        .all(|coefficient| coefficient > 0)
}

/// Returns the canonical frame's cumulative allocation charge, or `None` on overflow.
/// This applies to the current encoder, including its truthful arena hints and packed IDs.
/// It does not certify arbitrary protobuf bytes or a later relay's modified envelope.
#[doc(hidden)]
#[must_use]
pub fn canonical_event_decode_bytes(event: &pb::ChildEvent, profile: &pb::ReceiverAllocationProfile) -> Option<u64> {
    let pb::ChildEvent {
        total_execution_micros: _,
        max_suspensions: _,
        feed_execution_micros: _,
        max_feed_duration_micros: _,
        max_turn_duration_micros: _,
        max_total_sleep_micros: _,
        restored_script_name,
        session_id,
        accepted_receiver_allocation_profile_version: _,
        kind,
    } = event;
    let mut cost = Cost { total: 0, profile };
    cost.optional_text(restored_script_name.as_deref())?;
    if let Some(id) = session_id {
        cost.bytes(id.len())?;
    }
    if let Some(kind) = kind {
        match kind {
            pb::child_event::Kind::Print(print) => {
                cost.repeated(print.segments.len(), u64::from(profile.print_segment_slot_bytes))?;
                for segment in &print.segments {
                    cost.bytes(segment.text.len())?;
                }
            }
            pb::child_event::Kind::FunctionCall(call) => {
                cost.bytes(call.function_name.len())?;
                cost.arena(&call.values)?;
                cost.ids(&call.args)?;
                cost.pairs(call.kwargs.len())?;
                if call.object_id.is_some() {
                    cost.bytes(16)?;
                }
                if let Some(position) = &call.position {
                    cost.bytes(position.filename.len())?;
                }
            }
            pb::child_event::Kind::OsCall(call) => cost.os_call(call)?,
            pb::child_event::Kind::NameLookup(lookup) => {
                cost.bytes(lookup.name.len())?;
                if let Some(id) = &lookup.object_id {
                    cost.bytes(id.data.len())?;
                }
                cost.position(lookup.position.as_ref())?;
            }
            pb::child_event::Kind::ResolveFutures(futures) => {
                cost.repeated(futures.pending_call_ids.len(), 4)?;
                cost.position(futures.position.as_ref())?;
            }
            pb::child_event::Kind::Complete(complete) => {
                if let Some(arena) = &complete.values {
                    cost.arena(arena)?;
                }
            }
            pb::child_event::Kind::Error(error) => {
                if let Some(exception) = &error.exception {
                    cost.exception(exception)?;
                }
            }
            pb::child_event::Kind::TypingError(error) => cost.bytes(error.diagnostics.len())?,
            pb::child_event::Kind::DumpResult(dump) => cost.bytes(dump.state.len())?,
            pb::child_event::Kind::Ok(_) => {}
            pb::child_event::Kind::FatalError(error) => cost.bytes(error.message.len())?,
            pb::child_event::Kind::Shutdown(shutdown) => {
                if let Some(dump) = &shutdown.dump {
                    cost.bytes(dump.len())?;
                }
            }
        }
    }
    Some(cost.total)
}

/// Accumulates receiver charges while traversing each retained node and reference once.
struct Cost<'a> {
    total: u64,
    profile: &'a pb::ReceiverAllocationProfile,
}

impl Cost<'_> {
    /// Rejects overflow before the sender allocates an encoding buffer.
    fn add(&mut self, bytes: u64) -> Option<()> {
        self.total = self.total.checked_add(bytes)?;
        Some(())
    }

    /// Accounts for a fresh canonical string or byte payload.
    fn bytes(&mut self, bytes: usize) -> Option<()> {
        self.add(u64::try_from(bytes).ok()?)
    }

    /// Accounts for an optional string, including explicit empty presence.
    fn optional_text(&mut self, text: Option<&str>) -> Option<()> {
        self.bytes(text.map_or(0, str::len))
    }

    /// Accounts for deterministic incremental reservations of 4, 8, 16, ... slots.
    fn repeated(&mut self, len: usize, slot_bytes: u64) -> Option<()> {
        if len == 0 {
            Some(())
        } else {
            let slots = u64::try_from(len).ok()?.checked_next_power_of_two()?.max(4);
            self.add(slots.checked_mul(2)?.checked_sub(4)?.checked_mul(slot_bytes)?)
        }
    }

    /// Reference pairs pay two receiver reference allowances at each growth boundary.
    fn pairs(&mut self, len: usize) -> Option<()> {
        self.repeated(len, u64::from(self.profile.reference_slot_bytes).checked_mul(2)?)
    }

    /// Packed IDs reserve one slot per encoded varint byte, including over-reservation.
    fn ids(&mut self, ids: &[NodeId]) -> Option<()> {
        for id in ids {
            self.add(
                (encoded_len_varint(u64::from(id.0)) as u64)
                    .checked_mul(u64::from(self.profile.reference_slot_bytes))?,
            )?;
        }
        Some(())
    }

    /// Includes unreachable nodes because the canonical arena encodes all retained nodes.
    fn arena(&mut self, arena: &WireArena) -> Option<()> {
        self.add(
            u64::try_from(arena.0.len())
                .ok()?
                .checked_mul(u64::from(self.profile.node_slot_bytes))?,
        )?;
        for node in &arena.0 {
            match node {
                MontyNode::Ellipsis
                | MontyNode::NotImplemented
                | MontyNode::None
                | MontyNode::Bool(_)
                | MontyNode::Int(_)
                | MontyNode::Float(_)
                | MontyNode::Complex(_)
                | MontyNode::Date(_)
                | MontyNode::TimeDelta(_) => {}
                MontyNode::BigInt(value) => {
                    let magnitude = value.bits().checked_add(7)?.checked_div(8)?;
                    self.add(magnitude.max(1))?;
                    if magnitude > 0 {
                        self.add(magnitude.checked_add(7)?.checked_div(8)?.checked_mul(8)?.max(32))?;
                    }
                }
                MontyNode::String(text)
                | MontyNode::Path(text)
                | MontyNode::Decimal(text)
                | MontyNode::Repr(text)
                | MontyNode::Cycle(text) => self.bytes(text.len())?,
                MontyNode::Bytes(bytes) => self.bytes(bytes.len())?,
                MontyNode::List(ids) | MontyNode::Tuple(ids) | MontyNode::Set(ids) | MontyNode::FrozenSet(ids) => {
                    self.ids(ids)?;
                }
                MontyNode::NamedTuple {
                    type_name,
                    field_names,
                    values,
                } => {
                    self.bytes(type_name.len())?;
                    self.repeated(field_names.len(), u64::from(self.profile.string_slot_bytes))?;
                    for name in field_names {
                        self.bytes(name.len())?;
                    }
                    self.ids(values)?;
                }
                MontyNode::Dict(pairs) => self.pairs(pairs.len())?,
                MontyNode::ClassType(class) => {
                    self.add(u64::from(self.profile.class_box_bytes))?;
                    self.bytes(class.name.len())?;
                    self.bytes(16)?;
                    self.pairs(class.attrs.len())?;
                }
                MontyNode::ClassInstance { attrs, .. } => {
                    self.bytes(16)?;
                    self.pairs(attrs.len())?;
                }
                MontyNode::Module { name, attrs, .. } => {
                    self.bytes(name.len())?;
                    self.bytes(16)?;
                    self.pairs(attrs.len())?;
                }
                MontyNode::Type(value) => self.add(display_bytes(value)?)?,
                MontyNode::BuiltinFunction(value) => self.add(display_bytes(value)?)?,
                MontyNode::Exception { exc_type, arg } => {
                    self.add(display_bytes(exc_type)?)?;
                    self.optional_text(arg.as_deref())?;
                }
                MontyNode::FileHandle(handle) => {
                    self.bytes(handle.path.len())?;
                    self.bytes(handle.mode.as_str().len())?;
                }
                MontyNode::Function { name, docstring } => {
                    self.bytes(name.len())?;
                    self.optional_text(docstring.as_deref())?;
                }
                MontyNode::Time(time) => self.optional_text(time.timezone_name.as_deref())?,
                MontyNode::DateTime(time) => self.optional_text(time.timezone_name.as_deref())?,
                MontyNode::TimeZone(zone) => self.optional_text(zone.name.as_deref())?,
            }
        }
        Some(())
    }

    /// Positions contain only one allocating field.
    fn position(&mut self, position: Option<&pb::SourceRange>) -> Option<()> {
        self.optional_text(position.map(|position| position.filename.as_str()))
    }

    /// Counts every generated exception payload without rendering another traceback.
    fn exception(&mut self, exception: &pb::RaisedException) -> Option<()> {
        self.bytes(exception.exc_type.len())?;
        self.optional_text(exception.message.as_deref())?;
        self.repeated(exception.traceback.len(), u64::from(self.profile.traceback_slot_bytes))?;
        for frame in &exception.traceback {
            self.bytes(frame.filename.len())?;
            self.optional_text(frame.frame_name.as_deref())?;
            self.optional_text(frame.preview_line.as_deref())?;
        }
        if let Some(data) = &exception.data
            && let Some(kind) = &data.kind
        {
            match kind {
                pb::exc_data::Kind::Unicode(data) => {
                    self.bytes(data.encoding.len())?;
                    self.bytes(data.reason.len())?;
                    match &data.object {
                        Some(pb::unicode_error_data::Object::ObjectBytes(bytes)) => self.bytes(bytes.len())?,
                        Some(pb::unicode_error_data::Object::ObjectStr(text)) => self.bytes(text.len())?,
                        None => {}
                    }
                }
                pb::exc_data::Kind::Json(data) => {
                    self.bytes(data.msg.len())?;
                    self.optional_text(data.doc.as_deref())?;
                }
            }
        }
        Some(())
    }

    /// OS-call variants stay exhaustive so a new arm requires an accounting decision.
    fn os_call(&mut self, call: &pb::OsCall) -> Option<()> {
        if let Some(arena) = &call.values {
            self.arena(arena)?;
        }
        self.position(call.position.as_ref())?;
        if let Some(call) = &call.call {
            match call {
                pb::os_call::Call::Exists(path)
                | pb::os_call::Call::IsFile(path)
                | pb::os_call::Call::IsDir(path)
                | pb::os_call::Call::IsSymlink(path)
                | pb::os_call::Call::ReadText(path)
                | pb::os_call::Call::ReadBytes(path)
                | pb::os_call::Call::Stat(path)
                | pb::os_call::Call::Iterdir(path)
                | pb::os_call::Call::Resolve(path)
                | pb::os_call::Call::Absolute(path)
                | pb::os_call::Call::Unlink(path)
                | pb::os_call::Call::Rmdir(path) => self.bytes(path.len())?,
                pb::os_call::Call::WriteText(write) | pb::os_call::Call::AppendText(write) => {
                    self.bytes(write.path.len())?;
                    self.bytes(write.data.len())?;
                }
                pb::os_call::Call::WriteBytes(write) | pb::os_call::Call::AppendBytes(write) => {
                    self.bytes(write.path.len())?;
                    self.bytes(write.data.len())?;
                }
                pb::os_call::Call::Open(open) => {
                    self.bytes(open.path.len())?;
                    self.bytes(open.mode.len())?;
                }
                pb::os_call::Call::Mkdir(mkdir) => self.bytes(mkdir.path.len())?,
                pb::os_call::Call::Rename(rename) => {
                    self.bytes(rename.src.len())?;
                    self.bytes(rename.dst.len())?;
                }
                pb::os_call::Call::Getenv(getenv) => self.bytes(getenv.key.len())?,
                pb::os_call::Call::DateTimeNow(now) => {
                    if let Some(zone) = &now.tz {
                        self.optional_text(zone.name.as_deref())?;
                    }
                }
                pb::os_call::Call::Time(time) => self.bytes(time.caller.len())?,
                pb::os_call::Call::Import(import) => self.bytes(import.module.len())?,
                pb::os_call::Call::ImportStubs(stubs) => {
                    self.repeated(stubs.modules.len(), u64::from(self.profile.string_slot_bytes))?;
                    for module in &stubs.modules {
                        self.bytes(module.len())?;
                    }
                }
                pb::os_call::Call::Scan(scan) => {
                    self.bytes(scan.path.len())?;
                    self.repeated(scan.pattern.len(), u64::from(self.profile.string_slot_bytes))?;
                    for pattern in &scan.pattern {
                        self.bytes(pattern.len())?;
                    }
                }
                pb::os_call::Call::GetEnviron(_)
                | pb::os_call::Call::DateToday(_)
                | pb::os_call::Call::Urandom(_)
                | pb::os_call::Call::Sleep(_)
                | pb::os_call::Call::AsyncSleep(_)
                | pb::os_call::Call::SystemSleep(_)
                | pb::os_call::Call::AsyncSystemSleep(_) => {}
            }
        }
        Some(())
    }
}

/// Counts Display output without allocating rendered builtin names.
fn display_bytes(value: &impl Display) -> Option<u64> {
    let mut counter = ByteCounter(0);
    write!(&mut counter, "{value}").ok()?;
    Some(counter.0)
}

/// The formatter writes UTF-8 chunks, so their byte lengths match string decoding.
struct ByteCounter(u64);

impl Write for ByteCounter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0 = self
            .0
            .checked_add(u64::try_from(text.len()).map_err(|_| fmt::Error)?)
            .ok_or(fmt::Error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Cost, receiver_allocation_profile};

    #[test]
    fn cumulative_cost_overflow_is_rejected() {
        let profile = receiver_allocation_profile(1024);
        let mut cost = Cost {
            total: u64::MAX,
            profile: &profile,
        };
        assert_eq!(cost.add(1), None);
    }

    #[test]
    fn repeated_cost_overflow_is_rejected_at_the_coefficient_limit() {
        let profile = receiver_allocation_profile(1024);
        let mut cost = Cost {
            total: 0,
            profile: &profile,
        };
        assert_eq!(cost.repeated(usize::MAX, u64::from(u32::MAX)), None);
    }
}
