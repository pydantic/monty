//! Imports the host resolves: the [`ImportArgs`] an
//! [`OsFunctionCall::Import`](crate::OsFunctionCall::Import) carries, and the
//! `.py` file search a host runs when it has no host module of that name.
//!
//! The sandbox never reads module files itself. It sends one `import` call per
//! module, and the host answers with a host module, a [`ModuleFile`] it found
//! on the search path, or not-found. [`find_module_file`] is the search, in
//! CPython's order; hosts supply how one candidate is read.

use crate::{
    exceptions::{ExcType, MontyException},
    object::CallArgs,
    results::ExtFunctionResult,
    type_checking::is_identifier,
};

/// The arguments of an `import` call: the module and where its file may be.
///
/// Deserializes from a bare module name too, the form dumps written before
/// file imports store; such a call has an empty search path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "DumpedImportArgs")]
pub struct ImportArgs {
    /// The fully-qualified dotted module name, e.g. `pkg.sub`.
    pub module: String,
    /// Absolute virtual directories to search for the module's file, in order:
    /// the working directory for a top-level name, the package's `__path__`
    /// for a submodule. Empty means the host offers host modules only.
    pub search_path: Vec<String>,
}

impl ImportArgs {
    /// An import of `module` that only host modules can answer.
    #[must_use]
    pub fn new(module: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            search_path: Vec::new(),
        }
    }

    /// Reads the arguments back from the [`CallArgs`] a host sees for an
    /// `import` call: the module name, then its search path, which a call from
    /// a sandbox predating file imports leaves out. `None` when malformed.
    #[must_use]
    pub fn from_call_args(args: &CallArgs) -> Option<Self> {
        let mut args = args.args();
        let module = args.next()?.as_str()?.to_owned();
        let search_path = match args.next() {
            Some(dirs) => dirs
                .items()?
                .iter()
                .map(|dir| dir.as_str().map(str::to_owned))
                .collect::<Option<_>>()?,
            None => Vec::new(),
        };
        args.next().is_none().then_some(Self { module, search_path })
    }

    /// The files that may hold the module, in the order a search reads them:
    /// per directory, the package `name/__init__.py` before the module `name.py`.
    ///
    /// The call comes from an untrusted sandbox, so a name with a component
    /// that is not an identifier has no candidates, and a directory that is not
    /// absolute or holds a NUL byte is skipped. Paths are built one directory at
    /// a time: a long name times many directories never sits in memory at once.
    /// They are not normalized: mounts check them as sent, as for `open()`, and
    /// an `os` handler receives them normalized through [`OsFunctionCall::to_args`](crate::OsFunctionCall::to_args).
    pub fn candidates(&self) -> impl Iterator<Item = String> + '_ {
        let valid = self.module.split('.').all(is_identifier);
        let leaf = self.module.rsplit('.').next().unwrap_or(&self.module);
        self.search_path
            .iter()
            .filter(move |dir| valid && dir.starts_with('/') && !dir.contains('\0'))
            .flat_map(move |dir| {
                let dir = dir.strip_suffix('/').unwrap_or(dir);
                [format!("{dir}/{leaf}/__init__.py"), format!("{dir}/{leaf}.py")]
            })
    }
}

/// The two forms an `import` call's arguments take in a dump.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum DumpedImportArgs {
    /// A dump written before file imports: the module name alone.
    Name(String),
    Args {
        module: String,
        search_path: Vec<String>,
    },
}

impl From<DumpedImportArgs> for ImportArgs {
    fn from(dumped: DumpedImportArgs) -> Self {
        match dumped {
            DumpedImportArgs::Name(module) => Self::new(module),
            DumpedImportArgs::Args { module, search_path } => Self { module, search_path },
        }
    }
}

/// A module's `.py` file found by the host, which the sandbox runs as the module.
///
/// A `path` ending in `/__init__.py` makes the module a package.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModuleFile {
    /// The virtual path the file was read from: the module's `__file__`.
    pub path: String,
    /// The file's text.
    pub source: String,
}

/// The outcome of reading one candidate file during a [`find_module_file`] search.
#[derive(Debug)]
pub enum ModuleRead {
    /// The file's text: the search ends here.
    Text(String),
    /// No such file: the search moves on.
    Missing,
    /// No handler serves this path: the search moves on, as for a missing file.
    NotHandled,
    /// Reading failed for another reason: the import raises it.
    Error(MontyException),
}

impl ModuleRead {
    /// Classifies a `Path.read_text` failure: missing, a directory in the way,
    /// or a path outside anything the host serves moves on; anything else raises.
    #[must_use]
    pub fn from_exception(exc: MontyException) -> Self {
        match exc.exc_type() {
            ExcType::FileNotFoundError
            | ExcType::NotADirectoryError
            | ExcType::IsADirectoryError
            | ExcType::PermissionError => Self::Missing,
            _ => Self::Error(exc),
        }
    }

    /// Classifies a host handler's answer to `Path.read_text` for a candidate.
    #[must_use]
    pub fn from_result(result: ExtFunctionResult) -> Self {
        match result {
            ExtFunctionResult::Return(value) => match value.as_ref().as_str() {
                Some(text) => Self::Text(text.to_owned()),
                None => Self::Error(MontyException::new(
                    ExcType::RuntimeError,
                    Some(format!(
                        "invalid return type: Path.read_text for an import must return a str, not {}",
                        value.as_ref().type_name()
                    )),
                )),
            },
            ExtFunctionResult::Error(exc) => Self::from_exception(exc),
            ExtFunctionResult::NotFound(_) => Self::NotHandled,
            ExtFunctionResult::Future(_) | ExtFunctionResult::ModuleFile(_) => Self::Error(MontyException::new(
                ExcType::RuntimeError,
                Some("Path.read_text for an import must be answered with a str".to_owned()),
            )),
        }
    }
}

/// Searches `args`' candidates with `read`, returning the first file found as
/// a [`ExtFunctionResult::ModuleFile`], or the error a read raised.
///
/// `None` when no candidate exists: the host answers the import with not-found.
pub fn find_module_file(args: &ImportArgs, mut read: impl FnMut(&str) -> ModuleRead) -> Option<ExtFunctionResult> {
    for path in args.candidates() {
        match read(&path) {
            ModuleRead::Text(source) => return Some(ExtFunctionResult::ModuleFile(ModuleFile { path, source })),
            ModuleRead::Missing | ModuleRead::NotHandled => {}
            ModuleRead::Error(exc) => return Some(ExtFunctionResult::Error(exc)),
        }
    }
    None
}
