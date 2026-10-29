//! Imports of modules from `.py` files.
//!
//! An import the sandbox neither bundles nor caches is one `import` OS call
//! carrying the module's dotted name and the directories its file may be in:
//! the working directory for a top-level name, the package's `__path__` for a
//! submodule. The host does the whole search, checking its own modules first
//! as CPython checks built-in modules before `sys.path`, and answers with a
//! host module, the [`ModuleFile`] it found, or not-found. A found file is
//! compiled and run here as the module, so each module costs one round trip.

use std::sync::Arc;

use monty_types::{ExcType, ExtFunctionResult, ImportArgs, ModuleFile, OsFunctionCall};

use super::{FrameExit, FrameNamespace, VM, namespace::FrameGlobals};
use crate::{
    builtins::eval_exec::{SnippetSource, compile_and_push},
    bytecode::CallResult,
    exception_private::{ExcTypeExt, ExceptionRaise, RunError, RunResult, SimpleException},
    heap::{DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StringId,
    os_dispatch::PendingEffect,
    prepare::SnippetNames,
    types::{Dict, List, Module, str::allocate_string},
    value::Value,
    virtual_path::posix_join,
};

/// Where a top-level import searches, in order: CPython's `sys.path`.
// TODO(site-packages): configured entries (e.g. a read-only site-packages
// mount from `OsPolicy`), exposed read-only as `sys.path`.
#[derive(Clone, Copy)]
enum SearchEntry {
    /// The working directory, CPython's `''` entry.
    Cwd,
}

/// The search path of a top-level import.
const SEARCH_PATH: &[SearchEntry] = &[SearchEntry::Cwd];

/// What an import raises when the host has no module.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum OnMiss {
    /// `import name` and `import pkg.sub` raise `ModuleNotFoundError`.
    ModuleNotFound,
    /// `from pkg import sub` raises `ImportError` naming `pkg`'s file.
    CannotImport {
        /// The parent package's file, for the message.
        origin: String,
        /// Whether the parent's body was still running, which the message points out.
        partial: bool,
    },
}

/// An `import` call in flight, armed as its pending effect: a submodule's
/// suspended instruction does not name the module, and a miss's error depends
/// on the statement.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingImport {
    /// The fully-qualified dotted module name, e.g. `pkg.sub`.
    name: String,
    on_miss: OnMiss,
}

impl PendingImport {
    /// A top-level `import name`, which raises `ModuleNotFoundError` on a miss.
    pub(crate) fn top_level(name: String) -> Self {
        Self {
            name,
            on_miss: OnMiss::ModuleNotFound,
        }
    }

    /// Suspends with the `import` call for this module, searching `search_path`.
    fn suspend(self, search_path: Vec<String>) -> CallResult {
        CallResult::OsCallWithEffect {
            call: OsFunctionCall::Import(ImportArgs {
                module: self.name.clone(),
                search_path,
            }),
            effect: PendingEffect::Import(self),
        }
    }
}

/// A file module an import reaches through, read before the search for one of its submodules.
struct FileParent {
    name: String,
    origin: String,
    /// Whether its body is still running, as in a circular import.
    initializing: bool,
    /// The package's `__path__`; `None` for a plain module, which has no submodules.
    search_path: Option<Vec<String>>,
}

impl VM<'_> {
    /// `import name` for a name that is not bundled: the cached file module,
    /// else the host's `import` call, searching [`SEARCH_PATH`].
    pub(super) fn import_top_level(&mut self, name: &str) -> CallResult {
        if let Some(module) = self.modules.get(name, self.heap) {
            CallResult::Value(module)
        } else {
            let search_path = SEARCH_PATH
                .iter()
                .map(|entry| match entry {
                    SearchEntry::Cwd => self.env.cwd.to_string(),
                })
                .collect();
            PendingImport::top_level(name.to_owned()).suspend(search_path)
        }
    }

    /// `import pkg.sub` past a file module: the cached submodule, else the
    /// host's `import` call searching the package's `__path__`. `None` when
    /// `parent` is not a file module.
    pub(super) fn import_file_submodule(&mut self, parent: &Value, path_id: StringId) -> Option<RunResult<CallResult>> {
        let parent = self.file_parent(parent)?;
        let name = self.interns.get_str(path_id);
        Some(match parent.search_path {
            None => Err(ExcType::module_not_found_error(name, Some(&parent.name))),
            Some(dirs) => Ok(match self.modules.get(name, self.heap) {
                Some(module) => CallResult::Value(module),
                None => PendingImport::top_level(name.to_owned()).suspend(dirs),
            }),
        })
    }

    /// `from pkg import name` where `pkg` has no attribute `name`: for a file
    /// package, its submodule `name`. `None` when `parent` is not a file module.
    pub(super) fn import_file_attr(&mut self, parent: &Value, name_id: StringId) -> Option<RunResult<CallResult>> {
        let parent = self.file_parent(parent)?;
        let attr = self.interns.get_str(name_id);
        Some(match parent.search_path {
            None => Err(cannot_import(attr, &parent.name, &parent.origin, parent.initializing)),
            Some(dirs) => {
                let name = format!("{}.{attr}", parent.name);
                Ok(if let Some(module) = self.modules.get(&name, self.heap) {
                    CallResult::Value(module)
                } else {
                    let on_miss = OnMiss::CannotImport {
                        origin: parent.origin,
                        partial: parent.initializing,
                    };
                    PendingImport { name, on_miss }.suspend(dirs)
                })
            }
        })
    }

    /// Takes the armed `import` call, if the paused OS call is one.
    pub(crate) fn take_pending_import(&mut self) -> Option<PendingImport> {
        match self.pending_effect.take() {
            Some(PendingEffect::Import(pending)) => Some(pending),
            other => {
                self.pending_effect = other;
                None
            }
        }
    }

    /// Resumes an `import` call with the host's answer: a host module value,
    /// the module's file to run, an exception raised without carets, or
    /// not-found. Futures are rejected because imports cannot await.
    pub(crate) fn resume_import(
        &mut self,
        pending: PendingImport,
        result: ExtFunctionResult,
        eager_call_id: Option<u32>,
    ) -> Result<FrameExit, RunError> {
        match (result, eager_call_id) {
            (ExtFunctionResult::Return(obj), None) => self.resume(obj),
            (ExtFunctionResult::ModuleFile(file), None) => self.resume_import_file(pending.name, file),
            (ExtFunctionResult::Error(exc), None) => {
                let mut raise = ExceptionRaise::from(exc);
                raise.hide_caret = true;
                self.resume_with_exception(RunError::Exc(raise))
            }
            (ExtFunctionResult::NotFound(_), None) => {
                self.resume_with_exception(missed_import(&pending.name, pending.on_miss))
            }
            (ExtFunctionResult::Future(_), _) | (_, Some(_)) => {
                let message = format!("import of '{}' cannot be answered with a future", pending.name);
                self.resume_with_exception(SimpleException::new_msg(ExcType::RuntimeError, message).into())
            }
        }
    }

    /// Runs the file the host found as the module `name`.
    fn resume_import_file(&mut self, name: String, file: ModuleFile) -> Result<FrameExit, RunError> {
        let source = Arc::from(file.source);
        // Compiling is execution: it counts against the time limits.
        self.heap.tracker.on_execution_start();
        let result = self.run_file_module(name, &file.path, &source);
        self.heap.tracker.on_execution_stop();
        match result {
            Ok(()) => self.run_external(),
            Err(err) => self.resume_with_exception(err),
        }
    }

    /// After a module body returns: marks it initialised and binds a submodule
    /// on its parent package, as CPython does once the import succeeds. A no-op
    /// for an `__init__` frame.
    pub(super) fn finish_module_init(&mut self) -> RunResult<()> {
        let Some((module, name)) = self.file_module_on_stack() else {
            return Ok(());
        };
        self.modules.finish(&name, module);
        let Some((parent_name, leaf)) = name.rsplit_once('.') else {
            return Ok(());
        };
        let namespace = match self.modules.get_id(parent_name).map(|parent| self.heap.get(parent)) {
            Some(HeapData::Module(parent)) => parent.namespace(),
            _ => None,
        };
        let Some(namespace) = namespace else {
            return Ok(());
        };
        let key = allocate_string(leaf, self.heap);
        self.heap.inc_ref(module);
        let HeapReadOutput::Dict(dict) = self.heap.read(namespace) else {
            unreachable!("a file module's namespace is a dict")
        };
        let old = dict.set(key, Value::Ref(module), self)?;
        old.drop_with(self);
        Ok(())
    }

    /// After a frame raises: forgets the module whose body it was, so the next
    /// import retries the file. Returns whether the frame was a module body.
    pub(super) fn forget_failed_module(&mut self) -> bool {
        match self.file_module_on_stack() {
            Some((module, name)) => {
                self.modules.remove_failed(&name, module, self.heap);
                true
            }
            None => false,
        }
    }

    /// Runs `source`, read from `path`, as the module `name`: caches the module,
    /// pushes it as the import's result, then pushes its body's frame, which
    /// leaves it there on return (see `CallFrame::is_initializer`).
    fn run_file_module(&mut self, name: String, path: &str, source: &Arc<str>) -> RunResult<()> {
        let namespace = self.module_namespace(&name, path);
        let module = Module::file(name.clone(), namespace, path.to_owned());
        let module_id = self.heap.allocate(HeapData::Module(Box::new(module)));
        self.modules.insert(name, module_id, self.heap);
        self.push(Value::Ref(module_id));

        self.heap.inc_ref(namespace);
        let frame_namespace = Box::new(FrameNamespace::Snippet {
            globals: FrameGlobals::Dict(namespace),
            locals: None,
        });
        // TODO(site-packages): reuse compiled code across sessions; a large
        // package otherwise recompiles every module in every session.
        let kind = SnippetSource::File { path };
        match compile_and_push(kind, source, SnippetNames::NameOverDict, frame_namespace, self) {
            Ok(_) => {
                self.current_frame_mut().is_initializer = true;
                Ok(())
            }
            Err(err) => {
                self.forget_failed_module();
                self.pop().drop_with(self);
                Err(err)
            }
        }
    }

    /// A new module's globals: `__name__`, `__file__`, `__package__`, and
    /// `__path__` for a package, which a `path` ending in `/__init__.py` is.
    /// Returns an owned reference.
    fn module_namespace(&mut self, name: &str, path: &str) -> HeapId {
        let package_dir = path.strip_suffix("/__init__.py");
        let package = match (package_dir, name.rsplit_once('.')) {
            (Some(_), _) => name,
            (None, Some((parent, _))) => parent,
            (None, None) => "",
        };
        let mut entries = vec![
            ("__name__", allocate_string(name, self.heap)),
            ("__file__", allocate_string(path, self.heap)),
            ("__package__", allocate_string(package, self.heap)),
        ];
        if let Some(dir) = package_dir {
            let dir = allocate_string(dir, self.heap);
            entries.push((
                "__path__",
                Value::Ref(self.heap.allocate(HeapData::List(List::new(vec![dir])))),
            ));
        }
        let mut dict = Dict::new();
        for (key, value) in entries {
            let key = allocate_string(key, self.heap);
            // Few string keys into an empty dict: hashing cannot fail and growth is negligible.
            dict.set_without_growth_check(key, value, self)
                .expect("module dunder keys are strings, so hashing cannot fail");
        }
        self.heap.allocate(HeapData::Dict(dict))
    }

    /// `value` as a file module, with what an import of one of its submodules needs.
    fn file_parent(&self, value: &Value) -> Option<FileParent> {
        let id = value.ref_id()?;
        let HeapData::Module(module) = self.heap.get(id) else {
            return None;
        };
        let namespace = module.namespace()?;
        let HeapData::Dict(dict) = self.heap.get(namespace) else {
            return None;
        };
        let search_path = dict
            .get_by_str("__path__", self.heap, self.interns)
            .map(|path| self.search_path(path));
        let name = module.name_str(self.interns);
        Some(FileParent {
            initializing: self.modules.is_initializing(name, id),
            name: name.to_owned(),
            origin: module.origin().unwrap_or_default().to_owned(),
            search_path,
        })
    }

    /// A package's `__path__` (a list or tuple) as directories joined to the
    /// working directory, skipping anything that is not a `str`.
    fn search_path(&self, path: &Value) -> Vec<String> {
        let dirs = match path.ref_id().map(|id| self.heap.get(id)) {
            Some(HeapData::List(list)) => list.as_slice(),
            Some(HeapData::Tuple(tuple)) => tuple.as_slice(),
            _ => return Vec::new(),
        };
        // Matched directly: `to_str_heap` would format a discarded `TypeError` per non-string.
        dirs.iter()
            .filter_map(|dir| match dir {
                Value::InternString(id) => Some(self.interns.get_str(*id)),
                Value::Ref(id) => match self.heap.get(*id) {
                    HeapData::Str(s) => Some(s.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .map(|dir| posix_join(&self.env.cwd, dir))
            .collect()
    }

    /// The file module on top of the operand stack, where an import pushes it
    /// before running its body, with its dotted name. Checked on every
    /// `__init__` return too, so an empty cache answers before touching the heap.
    fn file_module_on_stack(&self) -> Option<(HeapId, String)> {
        if self.modules.is_empty() {
            return None;
        }
        let id = self.stack.last()?.ref_id()?;
        match self.heap.get(id) {
            HeapData::Module(module) if module.namespace().is_some() => {
                Some((id, module.name_str(self.interns).to_owned()))
            }
            _ => None,
        }
    }
}

/// The error for an import the host has no module for.
fn missed_import(name: &str, on_miss: OnMiss) -> RunError {
    match on_miss {
        OnMiss::ModuleNotFound => ExcType::module_not_found_error(name, None),
        OnMiss::CannotImport { origin, partial } => {
            let (parent, attr) = name.rsplit_once('.').expect("a submodule name is dotted");
            cannot_import(attr, parent, &origin, partial)
        }
    }
}

/// `from parent import attr` failing, noting a parent whose body is still running.
fn cannot_import(attr: &str, parent: &str, origin: &str, partial: bool) -> RunError {
    if partial {
        ExcType::cannot_import_name_partial(attr, parent, origin)
    } else {
        ExcType::cannot_import_name(attr, parent, Some(origin))
    }
}
