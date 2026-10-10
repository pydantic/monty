//! `os.scandir`, `os.walk`, `Path.walk`, `Path.glob` and `Path.rglob`.
//!
//! Each makes one [`OsFunctionCall::Scan`] for a bounded subtree, which a
//! [`ScanEffect`] turns into the Python result on resume: a `ScandirIterator`
//! of [`DirEntry`]s, a walk iterator, or a list iterator of glob matches.
//! Nothing below suspends again, so iteration needs no further host calls —
//! the price is that the whole subtree is read up front (see `limitations/os.md`).
//!
//! The objects share one heap payload, [`DirScan`], so the heap registers a
//! single variant for the family.

use std::{borrow::Cow, fmt::Write};

use monty_types::{
    MontyObject, MontyPath, OsFunctionCall, ResourceError, ResourceTracker, ScanArgs, StringRepr,
    scan::{
        EntryInfo, GlobSelector, ScanSource, ScanTree, indexed_size, normalize_relative, parse_scan_reply,
        split_literal_prefix,
    },
};
use serde::{Deserialize, Serialize};
use smallvec::smallvec;

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    os_dispatch::value_as_path_str,
    types::{
        LazyHeapSet, List, Path, PyTrait, Type, allocate_tuple,
        list::ListIterator,
        str::{allocate_string, string_repr_fmt},
    },
    value::{EitherStr, Value},
};

/// The heap payload for every object a directory scan produces.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum DirScan {
    /// An `os.DirEntry`.
    Entry(DirEntry),
    /// The `ScandirIterator` that `os.scandir()` returns.
    Scandir(ScandirIterator),
    /// The iterator `os.walk()` and `Path.walk()` return.
    Walk(WalkIterator),
}

impl DirScan {
    /// Whether this object can be part of a reference cycle: only a walk holds
    /// container references (its `onerror` callable and the yielded `dirnames`).
    pub(crate) fn is_gc_tracked(&self) -> bool {
        matches!(self, Self::Walk(_))
    }

    /// The Python type of this object.
    pub(crate) fn py_type(&self) -> Type {
        match self {
            Self::Entry(_) => Type::DirEntry,
            Self::Scandir(_) => Type::ScandirIterator,
            Self::Walk(_) => Type::WalkGenerator,
        }
    }

    /// The `__fspath__` of a `DirEntry`, `None` for the iterators.
    pub(crate) fn fspath(&self) -> Option<&str> {
        match self {
            Self::Entry(entry) => Some(&entry.path),
            Self::Scandir(_) | Self::Walk(_) => None,
        }
    }

    /// Invokes `on_child` for each heap id this object owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Self::Walk(walk) = self {
            if let Value::Ref(id) = &walk.onerror {
                on_child(*id);
            }
            if let Some(pending) = &walk.pending {
                on_child(pending.dirnames);
            }
        }
    }
}

impl HeapItem for DirScan {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Self::Walk(walk) = self {
            walk.onerror.py_dec_ref_ids(stack);
            if let Some(pending) = walk.pending.take() {
                stack.push(pending.dirnames);
            }
        }
    }
}

/// One `os.scandir()` result, answering its predicates from the scan without
/// another host call (`stat()` is the exception).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DirEntry {
    /// The entry's file name.
    name: String,
    /// `os.path.join(scandir_path, name)`, as CPython spells it.
    path: String,
    /// What the scan saw.
    info: EntryInfo,
}

/// The iterator `os.scandir()` returns: the listing, read in one call.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ScandirIterator {
    /// The directory as the caller spelled it, joined onto each entry's name.
    dir: String,
    /// The directory's children, sorted by name.
    entries: Vec<(String, EntryInfo)>,
    /// Index of the next entry; `close()` moves it to the end.
    position: usize,
}

/// `os.walk()` / `Path.walk()`: CPython's walk loop run over a prefetched tree.
///
/// A top-down walk keeps the `dirnames` list it last yielded and reads it back
/// on the next step, so pruning it (`dirnames[:] = ...`) works as in CPython —
/// it only saves sandbox work, since the host already read the whole tree.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct WalkIterator {
    /// The scanned subtree.
    tree: ScanTree,
    /// Whether this is `Path.walk()`, which yields `Path`s and lists symlinks
    /// to directories among the files unless it follows them.
    path_flavor: bool,
    /// Yield each directory before its subdirectories.
    topdown: bool,
    /// Descend into symlinked directories.
    followlinks: bool,
    /// Called with each `OSError`; `None` ignores them. Owned.
    onerror: Value,
    /// Steps still to take; the last is next.
    stack: Vec<WalkStep>,
    /// The directory last yielded top-down and its `dirnames` list (owned).
    pending: Option<PendingDirnames>,
    /// The host's error for the root, reported on the first step.
    root_error: Option<SimpleException>,
}

/// A step of [`WalkIterator`], mirroring CPython's `os.walk` stack.
#[derive(Debug, Serialize, Deserialize)]
enum WalkStep {
    /// List a directory: its normalized path relative to the scan root, `None`
    /// for a `dirnames` entry that left the scanned tree, and as yielded.
    Visit { relative: Option<String>, spelled: String },
    /// Yield a bottom-up result whose subdirectories are done.
    Yield {
        spelled: String,
        dirs: Vec<String>,
        files: Vec<String>,
    },
}

/// The `dirnames` list a top-down walk yielded, read back to decide where to descend.
#[derive(Debug, Serialize, Deserialize)]
struct PendingDirnames {
    /// The yielded directory relative to the scan root.
    relative: String,
    /// The yielded directory as spelled.
    spelled: String,
    /// The yielded `dirnames` list. Owned.
    dirnames: HeapId,
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, DirScan> {
    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).py_type()
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_is_iterable(&self, vm: &VM<'h>) -> bool {
        !matches!(self.get(vm.heap), DirScan::Entry(_))
    }

    fn py_is_iterator(&self, vm: &VM<'h>) -> bool {
        self.py_is_iterable(vm)
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        if self.py_is_iterable(vm) {
            Ok(self.clone_value(vm.heap))
        } else {
            Err(ExcType::type_error_not_iterable(&self.py_type_name(vm)))
        }
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match self.get_mut(vm.heap) {
            DirScan::Entry(_) => Err(ExcType::type_error_not_iterator(&self.py_type_name(vm))),
            DirScan::Scandir(iter) => {
                let Some((name, info)) = iter.entries.get(iter.position).cloned() else {
                    return Ok(None);
                };
                iter.position += 1;
                let path = os_path_join(&iter.dir, &name);
                let entry = DirScan::Entry(DirEntry { name, path, info });
                Ok(Some(Value::Ref(vm.heap.allocate(HeapData::DirScan(Box::new(entry))))))
            }
            DirScan::Walk(_) => walk_next(self, vm),
        }
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        match self.get(vm.heap) {
            DirScan::Entry(entry) => {
                f.write_str("<DirEntry ")?;
                string_repr_fmt(&entry.name, f)?;
                Ok(f.write_char('>')?)
            }
            DirScan::Scandir(_) | DirScan::Walk(_) => self.py_default_repr_fmt(f, vm),
        }
    }

    fn py_is_context_manager(&self, vm: &VM<'h>) -> bool {
        matches!(self.get(vm.heap), DirScan::Scandir(_))
    }

    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        if self.py_is_context_manager(vm) {
            Ok(CallResult::Value(self.clone_value(vm.heap)))
        } else {
            Err(ExcType::attribute_error(self.py_type_name(vm), "__enter__"))
        }
    }

    fn py_exit(&mut self, vm: &mut VM<'h>, _exc: Option<HeapId>) -> RunResult<CallResult> {
        match self.get_mut(vm.heap) {
            DirScan::Scandir(iter) => {
                iter.position = iter.entries.len();
                Ok(CallResult::Value(Value::None))
            }
            DirScan::Entry(_) | DirScan::Walk(_) => Err(ExcType::attribute_error(self.py_type_name(vm), "__exit__")),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let value = match (self.get(vm.heap), attr.static_string(vm.interns)) {
            (DirScan::Entry(entry), Some(StaticStrings::Name)) => allocate_string(entry.name.as_str(), vm.heap),
            (DirScan::Entry(entry), Some(StaticStrings::Path)) => allocate_string(entry.path.as_str(), vm.heap),
            _ => return Err(ExcType::attribute_error(self.py_type_name(vm), attr.as_str(vm.interns))),
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        value.drop_with(vm);
        let readonly = matches!(self.get(vm.heap), DirScan::Entry(_))
            && matches!(
                name.static_string(vm.interns),
                Some(StaticStrings::Name | StaticStrings::Path)
            );
        if readonly {
            Err(ExcType::attribute_error_readonly())
        } else {
            Err(ExcType::attribute_error_no_setattr(
                &self.py_type_name(vm),
                name.as_str(vm.interns),
            ))
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let method = attr.static_string(vm.interns);
        match (self.get(vm.heap), method) {
            (DirScan::Entry(entry), Some(method)) if ENTRY_METHODS.contains(&method) => {
                let (info, path) = (entry.info, entry.path.clone());
                entry_method(method, info, path, args, vm)
            }
            (DirScan::Scandir(_), Some(StaticStrings::Close)) => {
                args.check_zero_args("ScandirIterator.close", vm.heap)?;
                if let DirScan::Scandir(iter) = self.get_mut(vm.heap) {
                    iter.position = iter.entries.len();
                }
                Ok(CallResult::Value(Value::None))
            }
            _ => Err(ExcType::attribute_error_method(self.py_type_name(vm), attr, args, vm)),
        }
    }
}

impl DirScan {
    /// What the host sees when one crosses the boundary: its sandbox `repr`
    /// for an entry, a placeholder for the iterators.
    pub(crate) fn boundary_repr(&self) -> String {
        match self {
            Self::Entry(entry) => format!("<DirEntry {}>", StringRepr(&entry.name)),
            Self::Scandir(_) => "<posix.ScandirIterator object>".to_owned(),
            Self::Walk(_) => "<generator object>".to_owned(),
        }
    }
}

/// The `DirEntry` methods [`entry_method`] answers.
const ENTRY_METHODS: [StaticStrings; 6] = [
    StaticStrings::IsDir,
    StaticStrings::IsFile,
    StaticStrings::IsSymlink,
    StaticStrings::IsJunction,
    StaticStrings::StatMethod,
    StaticStrings::Fspath,
];

/// Runs a `DirEntry` method from what the scan saw; only `stat()` asks the host.
fn entry_method(
    method: StaticStrings,
    info: EntryInfo,
    path: String,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    let value = match method {
        StaticStrings::IsDir => {
            let IsDirArgs { follow_symlinks } = IsDirArgs::from_args(args, vm)?;
            Value::Bool(info.is_dir && (follow_symlinks.bool() || !info.is_symlink))
        }
        StaticStrings::IsFile => {
            let IsFileArgs { follow_symlinks } = IsFileArgs::from_args(args, vm)?;
            Value::Bool(info.is_file && (follow_symlinks.bool() || !info.is_symlink))
        }
        StaticStrings::IsSymlink => {
            args.check_zero_args("is_symlink", vm.heap)?;
            Value::Bool(info.is_symlink)
        }
        StaticStrings::IsJunction => {
            args.check_zero_args("is_junction", vm.heap)?;
            Value::Bool(false)
        }
        StaticStrings::StatMethod => {
            let StatArgs { follow_symlinks } = StatArgs::from_args(args, vm)?;
            // `lstat` of anything but a link is its `stat`.
            return if follow_symlinks.bool() || !info.is_symlink {
                Ok(CallResult::OsCall(OsFunctionCall::Stat(MontyPath::new(path))))
            } else {
                Err(ExcType::not_implemented_os_arg(Some("stat"), "follow_symlinks"))
            };
        }
        _ => {
            args.check_zero_args("__fspath__", vm.heap)?;
            allocate_string(path, vm.heap)
        }
    };
    Ok(CallResult::Value(value))
}

/// `DirEntry.is_dir(*, follow_symlinks=True)`.
#[derive(FromArgs)]
#[from_args(name = "is_dir", style = c_named)]
struct IsDirArgs {
    #[from_args(kw_only, default = LaxBool::new(true))]
    follow_symlinks: LaxBool,
}

/// `DirEntry.is_file(*, follow_symlinks=True)`.
#[derive(FromArgs)]
#[from_args(name = "is_file", style = c_named)]
struct IsFileArgs {
    #[from_args(kw_only, default = LaxBool::new(true))]
    follow_symlinks: LaxBool,
}

/// `DirEntry.stat(*, follow_symlinks=True)`.
#[derive(FromArgs)]
#[from_args(name = "stat", style = c_named)]
struct StatArgs {
    #[from_args(kw_only, default = LaxBool::new(true))]
    follow_symlinks: LaxBool,
}

/// Takes one step of a walk: prunes by the last `dirnames`, then pops steps
/// until one yields, reporting listing errors to `onerror` on the way.
///
/// No heap borrow is held across the `onerror` call, which re-enters Python.
fn walk_next<'h>(this: &mut HeapObjectRead<'h, DirScan>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let result = walk_step(this, vm);
    if result.is_err() {
        // An exception escaping a generator ends it, so a caught one must not
        // leave the remaining directories for a later `next()`.
        let walk = walk_mut(this, vm);
        walk.stack.clear();
        let pending = walk.pending.take();
        if let Some(pending) = pending {
            Value::Ref(pending.dirnames).drop_with(vm);
        }
    }
    result
}

/// The body of [`walk_next`], which ends the walk if this raises.
fn walk_step<'h>(this: &mut HeapObjectRead<'h, DirScan>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    loop {
        if let Some(pending) = walk_mut(this, vm).pending.take() {
            descend_into_dirnames(this, pending, vm)?;
        }
        let action = walk_mut(this, vm).step();
        match action {
            WalkAction::Done => return Ok(None),
            WalkAction::Error(error) => {
                let onerror = match this.get(vm.heap) {
                    DirScan::Walk(walk) => walk.onerror.clone_with_heap(vm.heap),
                    DirScan::Entry(_) | DirScan::Scandir(_) => unreachable!("walk_next is only called on a walk"),
                };
                defer_drop!(onerror, vm);
                if !matches!(onerror, Value::None) {
                    let RunError::Exc(raise) = error else {
                        return Err(error);
                    };
                    let exc = Value::Ref(vm.heap.allocate(HeapData::Exception(raise.exc)));
                    let result = vm.evaluate_function("os.walk() onerror", onerror, ArgValues::One(exc))?;
                    result.drop_with(vm);
                }
            }
            WalkAction::Yield {
                relative,
                spelled,
                dirs,
                files,
            } => {
                let path_flavor = walk_mut(this, vm).path_flavor;
                let dirpath = if path_flavor {
                    Value::Ref(vm.heap.allocate(HeapData::Path(Path::new(spelled.clone()))))
                } else {
                    allocate_string(spelled.as_str(), vm.heap)
                };
                let dirs = string_list(dirs, vm);
                let files = string_list(files, vm);
                if let (Some(relative), Value::Ref(dirnames)) = (relative, &dirs) {
                    vm.heap.inc_ref(*dirnames);
                    walk_mut(this, vm).pending = Some(PendingDirnames {
                        relative,
                        spelled,
                        dirnames: *dirnames,
                    });
                }
                return Ok(Some(allocate_tuple(smallvec![dirpath, dirs, files], vm.heap)));
            }
        }
    }
}

/// Queues the subdirectories still named in a top-down walk's yielded `dirnames`.
///
/// Names may be `str` or path-like, as `os.path.join` takes them. Like CPython,
/// a name that is not a directory is still visited (and reported to `onerror`);
/// a symlink is skipped unless the walk follows links. A name is looked up in
/// the tree normalized, as it is stored; one leaving the tree (absolute, or
/// `..` above the top) is visited as missing, where CPython would list it.
fn descend_into_dirnames<'h>(
    this: &mut HeapObjectRead<'h, DirScan>,
    pending: PendingDirnames,
    vm: &mut VM<'h>,
) -> RunResult<()> {
    let PendingDirnames {
        relative,
        spelled,
        dirnames,
    } = pending;
    let dirnames = Value::Ref(dirnames);
    defer_drop!(dirnames, vm);
    let Value::Ref(list_id) = dirnames else {
        unreachable!("constructed as a Ref above")
    };
    let HeapData::List(list) = vm.heap.get(*list_id) else {
        unreachable!("a walk's dirnames is always a list")
    };
    // Each name is copied, then joined onto the directory and spelled for its
    // step; charged first, since a list can repeat one long name many times.
    let mut total = 0_usize;
    for item in list.as_slice() {
        let Some(name) = value_as_path_str(item, vm.heap, vm.interns) else {
            return Err(ExcType::type_error(format!(
                "join() argument must be str, bytes, or os.PathLike object, not '{}'",
                item.py_type_name_heap(vm.heap, vm.interns)
            )));
        };
        let step = name.len() * 3 + relative.len() + spelled.len() + size_of::<WalkStep>();
        total = total.saturating_add(step);
    }
    vm.heap.tracker.check_allocation(total)?;
    let names: Vec<String> = list
        .as_slice()
        .iter()
        .filter_map(|item| value_as_path_str(item, vm.heap, vm.interns))
        .map(str::to_owned)
        .collect();
    let walk = walk_mut(this, vm);
    for name in names.iter().rev() {
        let child = if name.starts_with('/') {
            None
        } else {
            normalize_relative(&scan_join(&relative, name)).map(Cow::into_owned)
        };
        let is_symlink = child
            .as_deref()
            .and_then(|child| walk.tree.get(child))
            .is_some_and(|info| info.is_symlink);
        if walk.followlinks || !is_symlink {
            let spelled = walk.join_spelled(&spelled, name);
            walk.stack.push(WalkStep::Visit {
                relative: child,
                spelled,
            });
        }
    }
    Ok(())
}

/// The walk inside a `DirScan` known to be one.
fn walk_mut<'a, 'h>(this: &'a mut HeapObjectRead<'h, DirScan>, vm: &'a mut VM<'h>) -> &'a mut WalkIterator {
    match this.get_mut(vm.heap) {
        DirScan::Walk(walk) => walk,
        DirScan::Entry(_) | DirScan::Scandir(_) => unreachable!("walk_next is only called on a walk"),
    }
}

/// Allocates a list of `str`.
fn string_list(items: Vec<String>, vm: &VM<'_>) -> Value {
    let items = items.into_iter().map(|item| allocate_string(item, vm.heap)).collect();
    Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))))
}

/// What [`WalkIterator::step`] decided, carried out once its borrow ends.
enum WalkAction {
    /// The walk is over.
    Done,
    /// A directory could not be listed; report it to `onerror`.
    Error(RunError),
    /// Yield `(dirpath, dirnames, filenames)`; `relative` is set top-down,
    /// where the yielded `dirnames` decides what to visit next.
    Yield {
        relative: Option<String>,
        spelled: String,
        dirs: Vec<String>,
        files: Vec<String>,
    },
}

impl WalkIterator {
    /// Pops steps until one yields or fails, as CPython's `os.walk` loop does.
    fn step(&mut self) -> WalkAction {
        if let Some(exc) = self.root_error.take() {
            return WalkAction::Error(exc.into());
        }
        loop {
            let Some(step) = self.stack.pop() else {
                return WalkAction::Done;
            };
            let (relative, spelled) = match step {
                WalkStep::Yield { spelled, dirs, files } => {
                    return WalkAction::Yield {
                        relative: None,
                        spelled,
                        dirs,
                        files,
                    };
                }
                WalkStep::Visit { relative, spelled } => (relative, spelled),
            };
            let Some((relative, info)) = relative.and_then(|relative| {
                let info = self.tree.get(&relative)?;
                Some((relative, info))
            }) else {
                return WalkAction::Error(ExcType::file_not_found_error(&spelled));
            };
            if !info.is_dir {
                return WalkAction::Error(unlistable_error(info, &spelled));
            }
            let mut dirs = Vec::new();
            let mut files = Vec::new();
            let mut walk_into = Vec::new();
            for (name, info) in self.tree.children(&relative) {
                // `Path.walk` without `follow_symlinks` lists links to directories as files.
                let is_dir = info.is_dir && !(self.path_flavor && !self.followlinks && info.is_symlink);
                if is_dir {
                    dirs.push(name.clone());
                    if !self.topdown && (self.followlinks || !info.is_symlink) {
                        walk_into.push(name.clone());
                    }
                } else {
                    files.push(name.clone());
                }
            }
            if self.topdown {
                return WalkAction::Yield {
                    relative: Some(relative),
                    spelled,
                    dirs,
                    files,
                };
            }
            // Bottom-up: yield this directory once its subdirectories are done.
            let children: Vec<WalkStep> = walk_into
                .iter()
                .rev()
                .map(|name| WalkStep::Visit {
                    relative: Some(scan_join(&relative, name)),
                    spelled: self.join_spelled(&spelled, name),
                })
                .collect();
            self.stack.push(WalkStep::Yield { spelled, dirs, files });
            self.stack.extend(children);
        }
    }

    /// Spells a subdirectory the way the walk yields it: `os.path.join` for
    /// `os.walk`, `Path` joining (which drops a leading `./`) for `Path.walk`.
    fn join_spelled(&self, dir: &str, name: &str) -> String {
        if self.path_flavor {
            Path::new(dir.to_owned()).joinpath(name)
        } else {
            os_path_join(dir, name)
        }
    }
}

/// The error listing an entry that is not a directory raises: CPython's
/// `scandir` cannot open a dangling symlink at all, so that is
/// `FileNotFoundError`; anything else present is `NotADirectoryError`.
fn unlistable_error(info: EntryInfo, spelled: &str) -> RunError {
    if info.is_symlink && !info.is_dir && !info.is_file {
        ExcType::file_not_found_error(spelled)
    } else {
        ExcType::not_a_directory_error(spelled)
    }
}

/// `os.path.join(dir, name)` for a single relative `name`.
fn os_path_join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Joins a name onto a path relative to the scan root (`""` is the root).
fn scan_join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

// =============================================================================
// Building the calls and applying the replies.
// =============================================================================

/// What to build from a [`OsFunctionCall::Scan`] reply on resume.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum ScanEffect {
    /// `os.scandir(path)`; `spelled` is the path as given, joined onto each entry.
    Scandir { spelled: String },
    /// `os.walk()` / `Path.walk()`.
    Walk(WalkSetup),
    /// `Path.glob()` / `Path.rglob()`.
    Glob(GlobSetup),
}

/// The arguments of a walk, carried across the host call.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct WalkSetup {
    /// The top directory as spelled (`str(self)` for `Path.walk`).
    top: String,
    /// See [`WalkIterator::path_flavor`].
    path_flavor: bool,
    /// See [`WalkIterator::topdown`].
    topdown: bool,
    /// See [`WalkIterator::followlinks`].
    followlinks: bool,
    /// Owned until it moves into the iterator, or released with the effect.
    onerror: Value,
}

/// The parsed pattern of a glob, carried across the host call.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GlobSetup {
    /// The receiver joined with the pattern's literal prefix: the scan root as spelled.
    base: String,
    /// The parts after the prefix; empty when the whole pattern was literal.
    parts: Vec<String>,
    /// For an all-literal pattern, whether it ended in `/` and so names a directory.
    require_dir: bool,
    /// The pattern's last part, which decides how `pathlib` strips a trailing `/`.
    last_part: String,
    /// CPython's `case_sensitive` argument: `None` is the platform default.
    case_sensitive: Option<bool>,
    /// Whether `**` descends into symlinked directories.
    recurse_symlinks: bool,
}

impl ScanEffect {
    /// The Python operation, for error messages.
    pub(crate) fn operation_name(&self) -> &'static str {
        match self {
            Self::Scandir { .. } => "os.scandir",
            Self::Walk(walk) if walk.path_flavor => "Path.walk",
            Self::Walk(_) => "os.walk",
            Self::Glob(_) => "Path.glob",
        }
    }

    /// Whether the effect consumes `error` from the host instead of raising
    /// it: walks report an `OSError` to `onerror` and globs swallow it, as
    /// CPython does for its `scandir` failures. `os.scandir` raises.
    pub(crate) fn absorbs(&self, error: &RunError) -> bool {
        !matches!(self, Self::Scandir { .. })
            && matches!(error, RunError::Exc(raise) if raise.exc.exc_type().is_subclass_of(ExcType::OSError))
    }

    /// Builds the result from the host's reply, or from an error [`Self::absorbs`] accepted.
    pub(crate) fn apply(self, reply: Result<MontyObject, RunError>, vm: &mut VM<'_>) -> RunResult<Value> {
        // The reply bypasses `to_value`, so charge the indexed copy here. A host's
        // reply is bounded by its own budget; this keeps a large one from
        // overrunning the sandbox's limits before the next instruction checkpoint.
        if let Ok(reply) = &reply
            && let Err(err) = vm.heap.tracker.check_allocation(indexed_size(reply))
        {
            self.release(vm.heap);
            return Err(err.into());
        }
        let tree = match reply {
            Ok(reply) => match parse_scan_reply(&reply) {
                Ok(entries) => Ok(ScanTree::new(entries)),
                Err(description) => {
                    let message = format!("invalid return type: {}: {description}", self.operation_name());
                    self.release(vm.heap);
                    return Err(SimpleException::new_msg(ExcType::RuntimeError, message).into());
                }
            },
            Err(RunError::Exc(raise)) => Err(raise.exc),
            Err(other) => {
                self.release(vm.heap);
                return Err(other);
            }
        };
        if let Err(err) = vm.heap.tracker.check_time() {
            self.release(vm.heap);
            return Err(err.into());
        }
        match self {
            Self::Scandir { spelled } => {
                let tree = tree.map_err(RunError::from)?;
                if !tree.root().is_dir {
                    return Err(unlistable_error(tree.root(), &spelled));
                }
                let entries = tree.children("").to_vec();
                let iter = DirScan::Scandir(ScandirIterator {
                    dir: spelled,
                    entries,
                    position: 0,
                });
                Ok(Value::Ref(vm.heap.allocate(HeapData::DirScan(Box::new(iter)))))
            }
            Self::Walk(setup) => {
                let (tree, root_error) = match tree {
                    Ok(tree) => (tree, None),
                    Err(exc) => (ScanTree::default(), Some(exc)),
                };
                let stack = if root_error.is_some() {
                    Vec::new()
                } else {
                    vec![WalkStep::Visit {
                        relative: Some(String::new()),
                        spelled: setup.top,
                    }]
                };
                let walk = DirScan::Walk(WalkIterator {
                    tree,
                    path_flavor: setup.path_flavor,
                    topdown: setup.topdown,
                    followlinks: setup.followlinks,
                    onerror: setup.onerror,
                    stack,
                    pending: None,
                    root_error,
                });
                Ok(Value::Ref(vm.heap.allocate(HeapData::DirScan(Box::new(walk)))))
            }
            Self::Glob(setup) => {
                let matches = match tree {
                    Ok(tree) => setup.select(tree, &vm.heap.tracker)?,
                    Err(_) => Vec::new(),
                };
                vm.heap.tracker.check_time()?;
                let total: usize = matches.iter().map(|path| path.len() + size_of::<Value>()).sum();
                vm.heap.tracker.check_allocation(total)?;
                let paths = matches
                    .into_iter()
                    .map(|path| Value::Ref(vm.heap.allocate(HeapData::Path(Path::new(path)))))
                    .collect();
                let list = vm.heap.allocate(HeapData::List(List::new(paths)));
                Ok(Value::Ref(
                    vm.heap.allocate(HeapData::ListIterator(ListIterator::new(list))),
                ))
            }
        }
    }

    /// Drops what the effect owns when it will never be applied.
    pub(crate) fn release(self, heap: &mut impl ContainsHeap) {
        if let Self::Walk(setup) = self {
            setup.onerror.drop_with(heap);
        }
    }
}

impl GlobSetup {
    /// Runs the selector over the reply and spells each match as `pathlib` does.
    ///
    /// The matches can dwarf the reply they are drawn from (`**/*/**` yields
    /// every entry once per ancestor), so the selector polls `tracker` as it
    /// reads, raising `MemoryError` as they grow rather than at the hard ceiling.
    fn select(&self, tree: ScanTree, tracker: &ResourceTracker) -> Result<Vec<String>, ResourceError> {
        if self.parts.is_empty() {
            // A host that omitted the root has not shown that it exists.
            let root = tree.described_root();
            let exists = root.is_some_and(|info| info.is_dir || !self.require_dir);
            return Ok(if exists { vec![self.base.clone()] } else { Vec::new() });
        }
        let selector = GlobSelector::new(&self.parts, self.case_sensitive, self.recurse_symlinks);
        let mut tree = TrackedTree {
            tree,
            tracker,
            visits: 0,
        };
        let matches = selector.select(&mut tree)?;
        let base = Path::new(self.base.clone());
        Ok(matches
            .into_iter()
            .map(|relative| {
                let relative = match self.last_part.as_str() {
                    "" | "**" => relative.strip_suffix('/').unwrap_or(&relative),
                    _ => relative.as_str(),
                };
                if relative.is_empty() {
                    base.as_str().to_owned()
                } else {
                    base.joinpath(relative)
                }
            })
            .collect())
    }
}

/// A reply's tree read under the sandbox's limits: the selector's work and
/// the matches it collects grow with every entry visited, so each visit is a
/// poll of the memory and time limits.
struct TrackedTree<'a> {
    tree: ScanTree,
    tracker: &'a ResourceTracker,
    /// Visits so far, which paces the polls.
    visits: usize,
}

impl ScanSource for TrackedTree<'_> {
    type Error = ResourceError;

    fn list(&mut self, dir: &str) -> Result<Option<Vec<(String, EntryInfo)>>, ResourceError> {
        let Ok(listing) = self.tree.list(dir);
        Ok(listing)
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, ResourceError> {
        let Ok(info) = self.tree.lookup(path);
        Ok(info)
    }

    fn visit(&mut self) -> Result<(), ResourceError> {
        self.visits += 1;
        self.tracker.check_memory_time_every(self.visits)
    }
}

/// Starts `os.scandir(path)`: a depth-one scan, answered with a `ScandirIterator`.
pub(crate) fn scandir_call(path: MontyPath) -> CallResult {
    let spelled = path.as_str().to_owned();
    CallResult::OsCallWithEffect {
        call: OsFunctionCall::Scan(ScanArgs::listing(path, Some(1), false)),
        effect: ScanEffect::Scandir { spelled }.into(),
    }
}

/// Starts `os.walk()` / `Path.walk()`: a full scan, answered with a walk iterator.
///
/// Takes ownership of `onerror`.
pub(crate) fn walk_call(
    top: String,
    path_flavor: bool,
    topdown: bool,
    onerror: Value,
    followlinks: bool,
) -> CallResult {
    let call = OsFunctionCall::Scan(ScanArgs::listing(MontyPath::new(top.clone()), None, followlinks));
    CallResult::OsCallWithEffect {
        call,
        effect: ScanEffect::Walk(WalkSetup {
            top,
            path_flavor,
            topdown,
            followlinks,
            onerror,
        })
        .into(),
    }
}

/// Starts `Path.glob()` on the receiver `base` with pattern text `pattern`.
///
/// Parses the pattern as `pathlib` does, hoists its literal prefix into the
/// scan root, and scans what is left.
pub(crate) fn glob_call(
    base: &str,
    pattern: &str,
    case_sensitive: Option<bool>,
    recurse_symlinks: bool,
) -> RunResult<CallResult> {
    if pattern.starts_with('/') {
        return Err(
            SimpleException::new_msg(ExcType::NotImplementedError, "Non-relative patterns are unsupported").into(),
        );
    }
    let mut parts: Vec<String> = pattern
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    if parts.is_empty() {
        return Err(ExcType::value_error(format!(
            "Unacceptable pattern: {}",
            StringRepr(pattern)
        )));
    }
    if pattern.ends_with('/') {
        parts.push(String::new());
    }
    let last_part = parts.last().cloned().unwrap_or_default();
    let (prefix, rest) = split_literal_prefix(&parts, case_sensitive);
    let require_dir = prefix.ends_with('/');
    let prefix = prefix.trim_end_matches('/');
    let root = if prefix.is_empty() {
        base.to_owned()
    } else {
        Path::new(base.to_owned()).joinpath(prefix)
    };
    let rest = rest.to_vec();
    let args = if rest.is_empty() {
        ScanArgs::listing(MontyPath::new(root.clone()), Some(0), false)
    } else {
        ScanArgs::glob(
            MontyPath::new(root.clone()),
            rest.clone(),
            case_sensitive,
            recurse_symlinks,
        )
    };
    Ok(CallResult::OsCallWithEffect {
        call: OsFunctionCall::Scan(args),
        effect: ScanEffect::Glob(GlobSetup {
            base: root,
            parts: rest,
            require_dir,
            last_part,
            case_sensitive,
            recurse_symlinks,
        })
        .into(),
    })
}
