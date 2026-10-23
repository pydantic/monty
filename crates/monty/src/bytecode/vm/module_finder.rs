//! Imports of modules from `.py` files.
//!
//! A top-level import that is neither bundled nor cached asks the host's
//! `import` call first, as CPython checks built-in modules before `sys.path`.
//! Only when the host has no such module does it read candidate files, one
//! `Path.read_text` OS call at a time, so the host's ordinary filesystem
//! handling (mounts, overlays, a custom `os` handler) decides what `import`
//! can read, exactly as for `open()`. The first file found is compiled and run
//! as the module. [`ImportProbe`] carries the search across those suspensions.

use std::sync::Arc;

use monty_types::{ExcType, MontyObject, MontyPath, OsFunctionCall};

use super::{FrameExit, FrameNamespace, VM, namespace::FrameGlobals};
use crate::{
    builtins::eval_exec::{SnippetSource, compile_and_push},
    bytecode::CallResult,
    exception_private::{ExcTypeExt, RunError, RunResult, SimpleException},
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

/// Which file of a module a probe reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Candidate {
    /// `<dir>/<name>.py`
    Module,
    /// `<dir>/<name>/__init__.py`, a package.
    Package,
}

/// What an import raises when no directory has its file.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum OnMiss {
    /// `import name` and `import pkg.sub` raise `ModuleNotFoundError`.
    ModuleNotFound,
    /// `from pkg import sub` raises `ImportError` naming `pkg`'s file.
    CannotImport {
        /// The parent package's file, for the message.
        origin: String,
    },
}

/// A file import in progress: the pending effect of each `Path.read_text` probe.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ImportProbe {
    /// The fully-qualified dotted module name, e.g. `pkg.sub`.
    name: String,
    /// Directories to search, in order.
    dirs: Vec<String>,
    /// The directory in `dirs` this probe reads from.
    dir_index: usize,
    candidate: Candidate,
    on_miss: OnMiss,
}

impl ImportProbe {
    /// Suspends to read this probe's candidate file, the probe armed for the reply.
    fn suspend(self) -> CallResult {
        CallResult::OsCallWithEffect {
            call: self.call(),
            effect: PendingEffect::Import(self),
        }
    }

    /// The OS call that reads this probe's candidate file.
    fn call(&self) -> OsFunctionCall {
        OsFunctionCall::ReadText(MontyPath::new(self.path()))
    }

    /// The candidate file's virtual path.
    fn path(&self) -> String {
        let leaf = self.name.rsplit_once('.').map_or(self.name.as_str(), |(_, leaf)| leaf);
        let file = match self.candidate {
            Candidate::Module => format!("{leaf}.py"),
            Candidate::Package => format!("{leaf}/__init__.py"),
        };
        posix_join(&self.dirs[self.dir_index], &file)
    }

    /// The package directory a found `__init__.py` makes `__path__`.
    fn package_dir(&self) -> Option<String> {
        let path = self.path();
        (self.candidate == Candidate::Package).then(|| path.trim_end_matches("/__init__.py").to_owned())
    }

    /// Moves on to the next candidate after a miss; `false` once every candidate is read.
    // TODO(site-packages): batch the candidates into one host call; two round
    // trips per directory is too slow for a package that imports hundreds of modules.
    fn advance(&mut self) -> bool {
        match self.candidate {
            Candidate::Module => {
                self.candidate = Candidate::Package;
                true
            }
            Candidate::Package if self.dir_index + 1 < self.dirs.len() => {
                self.dir_index += 1;
                self.candidate = Candidate::Module;
                true
            }
            Candidate::Package => false,
        }
    }
}

/// A file module an import reaches through, read before the search for one of its submodules.
struct FileParent {
    name: String,
    origin: String,
    /// The package's `__path__`; `None` for a plain module, which has no submodules.
    search_path: Option<Vec<String>>,
}

impl VM<'_> {
    /// `import name` for a name that is not bundled: the cached file module,
    /// else the host's `import` call, which falls back to
    /// [`resume_import_from_files`](Self::resume_import_from_files).
    pub(super) fn import_top_level(&mut self, name: &str) -> CallResult {
        match self.modules.get(name, self.heap) {
            Some(module) => CallResult::Value(module),
            None => CallResult::OsCall(OsFunctionCall::Import(name.to_owned())),
        }
    }

    /// Resumes a top-level import the host has no module for: searches
    /// [`SEARCH_PATH`] for its file, raising `ModuleNotFoundError` if there is none.
    pub(crate) fn resume_import_from_files(&mut self, name: &str) -> Result<FrameExit, RunError> {
        let dirs = SEARCH_PATH
            .iter()
            .map(|entry| match entry {
                SearchEntry::Cwd => self.env.cwd.to_string(),
            })
            .collect();
        match start_search(name.to_owned(), dirs, OnMiss::ModuleNotFound) {
            Ok(probe) => {
                let call = probe.call();
                self.suspend_again(call, Some(PendingEffect::Import(probe)))
            }
            Err(err) => self.resume_with_exception(err),
        }
    }

    /// `import pkg.sub` past a file module: the cached submodule, else a search
    /// of the package's `__path__`. `None` when `parent` is not a file module.
    pub(super) fn import_file_submodule(&mut self, parent: &Value, path_id: StringId) -> Option<RunResult<CallResult>> {
        let parent = self.file_parent(parent)?;
        let name = self.interns.get_str(path_id);
        Some(match parent.search_path {
            None => Err(ExcType::module_not_found_error(name, Some(&parent.name))),
            Some(dirs) => match self.modules.get(name, self.heap) {
                Some(module) => Ok(CallResult::Value(module)),
                None => start_search(name.to_owned(), dirs, OnMiss::ModuleNotFound).map(ImportProbe::suspend),
            },
        })
    }

    /// `from pkg import name` where `pkg` has no attribute `name`: for a file
    /// package, its submodule `name`. `None` when `parent` is not a file module.
    pub(super) fn import_file_attr(&mut self, parent: &Value, name_id: StringId) -> Option<RunResult<CallResult>> {
        let parent = self.file_parent(parent)?;
        let attr = self.interns.get_str(name_id);
        Some(match parent.search_path {
            None => Err(ExcType::cannot_import_name(attr, &parent.name, Some(&parent.origin))),
            Some(dirs) => {
                let name = format!("{}.{attr}", parent.name);
                match self.modules.get(&name, self.heap) {
                    Some(module) => Ok(CallResult::Value(module)),
                    None => start_search(name, dirs, OnMiss::CannotImport { origin: parent.origin })
                        .map(ImportProbe::suspend),
                }
            }
        })
    }

    /// Resumes a probe the host answered with the file's text: runs it as the module.
    pub(super) fn resume_import_found(
        &mut self,
        probe: ImportProbe,
        reply: &MontyObject,
    ) -> Result<FrameExit, RunError> {
        let Some(source) = reply.as_ref().as_str() else {
            let msg = format!(
                "invalid return type: Path.read_text for an import must return a str, not {}",
                reply.as_ref().type_name()
            );
            return self.resume_with_exception(SimpleException::new_msg(ExcType::RuntimeError, msg).into());
        };
        let source = Arc::from(source);
        // Compiling is execution: it counts against the time limits.
        self.heap.tracker.on_execution_start();
        let result = self.run_file_module(probe, &source);
        self.heap.tracker.on_execution_stop();
        match result {
            Ok(()) => self.run_external(),
            Err(err) => self.resume_with_exception(err),
        }
    }

    /// Resumes a probe whose file is missing: reads the next candidate, or
    /// misses the import once there is none.
    pub(crate) fn resume_import_miss(&mut self, mut probe: ImportProbe) -> Result<FrameExit, RunError> {
        if probe.advance() {
            let call = probe.call();
            self.suspend_again(call, Some(PendingEffect::Import(probe)))
        } else {
            self.resume_with_exception(missed_import(&probe.name, probe.on_miss))
        }
    }

    /// Takes the armed import probe, if the paused OS call is one.
    pub(crate) fn take_import_probe(&mut self) -> Option<ImportProbe> {
        match self.pending_effect.take() {
            Some(PendingEffect::Import(probe)) => Some(probe),
            other => {
                self.pending_effect = other;
                None
            }
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

    /// After a module body raises: forgets the module so the next import
    /// retries the file. A no-op for an `__init__` frame.
    pub(super) fn forget_failed_module(&mut self) {
        if let Some((module, name)) = self.file_module_on_stack() {
            self.modules.remove_failed(&name, module, self.heap);
        }
    }

    /// Runs `source` as the module `probe` found: caches the module, pushes it
    /// as the import's result, then pushes its body's frame, which leaves it
    /// there on return (see `CallFrame::is_initializer`).
    fn run_file_module(&mut self, probe: ImportProbe, source: &Arc<str>) -> RunResult<()> {
        let path = probe.path();
        let namespace = self.module_namespace(&probe, &path);
        let module = Module::file(probe.name.clone(), namespace, path.clone());
        let module_id = self.heap.allocate(HeapData::Module(Box::new(module)));
        self.modules.insert(probe.name, module_id, self.heap);
        self.push(Value::Ref(module_id));

        self.heap.inc_ref(namespace);
        let frame_namespace = Box::new(FrameNamespace::Snippet {
            globals: FrameGlobals::Dict(namespace),
            locals: None,
        });
        // TODO(site-packages): reuse compiled code across sessions; a large
        // package otherwise recompiles every module in every session.
        let kind = SnippetSource::File { path: &path };
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
    /// `__path__` for a package. Returns an owned reference.
    fn module_namespace(&mut self, probe: &ImportProbe, path: &str) -> HeapId {
        let package_dir = probe.package_dir();
        let package = match (&package_dir, probe.name.rsplit_once('.')) {
            (Some(_), _) => probe.name.as_str(),
            (None, Some((parent, _))) => parent,
            (None, None) => "",
        };
        let mut entries = vec![
            ("__name__", allocate_string(probe.name.as_str(), self.heap)),
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
        let HeapData::Module(module) = self.heap.get(value.ref_id()?) else {
            return None;
        };
        let namespace = module.namespace()?;
        let HeapData::Dict(dict) = self.heap.get(namespace) else {
            return None;
        };
        let search_path = dict
            .get_by_str("__path__", self.heap, self.interns)
            .map(|path| self.search_path(path));
        Some(FileParent {
            name: module.name_str(self.interns).to_owned(),
            origin: module.origin().unwrap_or_default().to_owned(),
            search_path,
        })
    }

    /// A package's `__path__` (a list or tuple) as directories, skipping anything that is not a `str`.
    fn search_path(&self, path: &Value) -> Vec<String> {
        let dirs = match path.ref_id().map(|id| self.heap.get(id)) {
            Some(HeapData::List(list)) => list.as_slice(),
            Some(HeapData::Tuple(tuple)) => tuple.as_slice(),
            _ => return Vec::new(),
        };
        dirs.iter()
            .filter_map(|dir| dir.to_str_heap(self.heap, self.interns).ok())
            .map(str::to_owned)
            .collect()
    }

    /// The file module on top of the operand stack, where an import pushes it
    /// before running its body, with its dotted name.
    fn file_module_on_stack(&self) -> Option<(HeapId, String)> {
        let id = self.stack.last()?.ref_id()?;
        match self.heap.get(id) {
            HeapData::Module(module) if module.namespace().is_some() => {
                Some((id, module.name_str(self.interns).to_owned()))
            }
            _ => None,
        }
    }
}

/// The first probe of a search of `dirs` for `name`'s file, or the miss when there are none.
fn start_search(name: String, dirs: Vec<String>, on_miss: OnMiss) -> RunResult<ImportProbe> {
    if dirs.is_empty() {
        Err(missed_import(&name, on_miss))
    } else {
        Ok(ImportProbe {
            name,
            dirs,
            dir_index: 0,
            candidate: Candidate::Module,
            on_miss,
        })
    }
}

/// The error for an import no directory has a file for.
fn missed_import(name: &str, on_miss: OnMiss) -> RunError {
    match on_miss {
        OnMiss::ModuleNotFound => ExcType::module_not_found_error(name, None),
        OnMiss::CannotImport { origin } => {
            let (parent, attr) = name.rsplit_once('.').expect("a submodule name is dotted");
            ExcType::cannot_import_name(attr, parent, Some(&origin))
        }
    }
}

/// Whether the host's error answering a probe means its file is not there:
/// missing, a directory in the way, or outside anything the host serves.
pub(crate) fn is_missing_file(error: &RunError) -> bool {
    matches!(
        error,
        RunError::Exc(raise) if matches!(
            raise.exc.exc_type(),
            ExcType::FileNotFoundError
                | ExcType::NotADirectoryError
                | ExcType::IsADirectoryError
                | ExcType::PermissionError
        )
    )
}
