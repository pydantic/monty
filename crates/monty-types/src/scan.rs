//! Directory-tree scans: the one OS call behind `os.scandir`, `os.walk`,
//! `Path.walk`, `Path.glob` and `Path.rglob`.
//!
//! The sandbox cannot suspend part-way through an iterator, so it asks the host
//! for a whole bounded subtree in one [`OsFunctionCall::Scan`](crate::OsFunctionCall::Scan)
//! and walks or globs the reply itself.
//!
//! A glob pattern rides along so the host can prune, but the contract is only
//! that the host returns *at least* the entries the sandbox needs: every entry
//! [`GlobSelector`] visits when run against the real tree. Extra entries are
//! harmless because the sandbox re-runs the selector over the reply, so a host
//! that ignores the pattern and returns everything within `max_depth` (descending
//! symlinks when `follow_symlinks` says so) is still correct.
//! [`collect_scan`] is the pruning implementation hosts can share.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    convert::Infallible,
};

use crate::{
    graph::{MontyGraph, MontyNode, NodeId},
    object::MontyObject,
    os::MontyPath,
    unstable::{self, PushValue},
};

/// Arguments of a [`Scan`](crate::OsFunctionCall::Scan) call.
///
/// Hosts that ignore `pattern` honour `max_depth` and `follow_symlinks`; hosts
/// that prune with it use `pattern`, `case_sensitive` and `recurse_symlinks`
/// (see [`collect_scan`]). Construct with [`ScanArgs::listing`] or [`ScanArgs::glob`],
/// which keep the two views consistent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, monty_macros::ToArgs)]
pub struct ScanArgs {
    /// Root of the scan; reply paths are relative to it.
    pub path: MontyPath,
    /// Deepest entry to return: `Some(0)` is the root alone, `Some(1)` its
    /// children (`scandir`), `None` the whole tree.
    #[from_args(kw_only)]
    pub max_depth: Option<u32>,
    /// Whether to descend into symlinked directories when not pruning with `pattern`.
    #[from_args(kw_only)]
    pub follow_symlinks: bool,
    /// `Path.glob` pattern parts relative to `path`, as CPython parses them
    /// (a trailing `''` means "directories only"); a pruning hint only.
    #[from_args(kw_only)]
    pub pattern: Option<Vec<String>>,
    /// CPython's `case_sensitive` argument: `None` matches case-sensitively and
    /// looks literal parts up directly; an explicit value lists every part.
    #[from_args(kw_only)]
    pub case_sensitive: Option<bool>,
    /// Whether `**` in `pattern` descends into symlinked directories.
    #[from_args(kw_only)]
    pub recurse_symlinks: bool,
}

impl ScanArgs {
    /// A plain listing to `max_depth` (`os.scandir`, `os.walk`, `Path.walk`).
    #[must_use]
    pub fn listing(path: MontyPath, max_depth: Option<u32>, follow_symlinks: bool) -> Self {
        Self {
            path,
            max_depth,
            follow_symlinks,
            pattern: None,
            case_sensitive: None,
            recurse_symlinks: false,
        }
    }

    /// A `Path.glob` scan of the parts left after [`split_literal_prefix`].
    ///
    /// Derives the fallback `max_depth` and `follow_symlinks` a pattern-ignoring
    /// host needs to return a superset: wildcard parts other than the last may
    /// match a symlinked directory and then descend into it.
    #[must_use]
    pub fn glob(path: MontyPath, parts: Vec<String>, case_sensitive: Option<bool>, recurse_symlinks: bool) -> Self {
        let recursive = parts.iter().any(|part| part == "**");
        let depth = parts.iter().filter(|part| !is_special(part)).count();
        let descends_wildcard = parts
            .iter()
            .take(parts.len().saturating_sub(1))
            .any(|part| part != "**" && !is_special(part));
        Self {
            path,
            max_depth: if recursive {
                None
            } else {
                Some(u32::try_from(depth).unwrap_or(u32::MAX))
            },
            follow_symlinks: recurse_symlinks || descends_wildcard,
            pattern: Some(parts),
            case_sensitive,
            recurse_symlinks,
        }
    }
}

/// What a scan knows about one entry, with `os.DirEntry` semantics:
/// `is_dir` and `is_file` follow a symlink, `is_symlink` does not.
///
/// A broken symlink is `is_symlink` alone; something that is none of the three
/// (a socket, a FIFO) is all `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EntryInfo {
    /// The entry is, or links to, a directory.
    pub is_dir: bool,
    /// The entry is, or links to, a regular file.
    pub is_file: bool,
    /// The entry itself is a symbolic link.
    pub is_symlink: bool,
}

/// One entry of a scan reply: its path relative to the scan root (`""` for the
/// root itself, `/`-separated below it) and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    /// Relative path, normalized: no `.`, `..` or empty components.
    pub path: String,
    /// Kind of entry.
    pub info: EntryInfo,
}

/// Encodes a scan reply as the host returns it: a list of
/// `(path, is_dir, is_file, is_symlink)` tuples, including the root as `""`.
#[must_use]
pub fn scan_reply(entries: impl IntoIterator<Item = ScanEntry>) -> MontyObject {
    MontyObject::list(entries.into_iter().map(|entry| {
        MontyObject::tuple([
            MontyObject::string(entry.path),
            MontyObject::bool(entry.info.is_dir),
            MontyObject::bool(entry.info.is_file),
            MontyObject::bool(entry.info.is_symlink),
        ])
    }))
}

/// Decodes a host's scan reply (see [`scan_reply`]); entries may be tuples,
/// lists or named tuples, and their paths `str` or `Path`.
///
/// The error describes what was wrong for the sandbox's `RuntimeError`. Paths
/// are normalized, and one that is absolute or climbs above the root is refused.
pub fn parse_scan_reply(reply: &MontyObject) -> Result<Vec<ScanEntry>, String> {
    let MontyNode::List(ids) = unstable::root_node(reply) else {
        return Err(format!(
            "expected a list of (path, is_dir, is_file, is_symlink) tuples, got {}",
            reply.as_ref().type_name()
        ));
    };
    let (graph, _) = unstable::graph_parts(reply);
    ids.iter().map(|&id| parse_entry(graph, id)).collect()
}

/// Bytes [`parse_scan_reply`] and [`ScanTree::new`] allocate for `reply`, roughly:
/// each path is held three times (parsed, keyed, listed) plus per-entry overhead.
///
/// Lets the sandbox charge its memory limit before indexing a reply, without allocating.
#[must_use]
pub fn indexed_size(reply: &MontyObject) -> usize {
    /// Map slots, list slots and flags per entry, generously.
    const ENTRY_OVERHEAD: usize = 160;
    let MontyNode::List(ids) = unstable::root_node(reply) else {
        return 0;
    };
    let (graph, _) = unstable::graph_parts(reply);
    ids.iter()
        .map(|&id| {
            let path_len = match graph.node(id) {
                MontyNode::Tuple(items) | MontyNode::List(items) | MontyNode::NamedTuple { values: items, .. } => {
                    match items.first().map(|&path| graph.node(path)) {
                        Some(MontyNode::String(path) | MontyNode::Path(path)) => path.len(),
                        _ => 0,
                    }
                }
                _ => 0,
            };
            path_len.saturating_mul(3).saturating_add(ENTRY_OVERHEAD)
        })
        .fold(0, usize::saturating_add)
}

/// Decodes one `(path, is_dir, is_file, is_symlink)` tuple.
fn parse_entry(graph: &MontyGraph, id: NodeId) -> Result<ScanEntry, String> {
    let invalid = || {
        format!(
            "expected (path, is_dir, is_file, is_symlink) tuples, got {}",
            graph.type_name(id)
        )
    };
    let (MontyNode::Tuple(items) | MontyNode::List(items) | MontyNode::NamedTuple { values: items, .. }) =
        graph.node(id)
    else {
        return Err(invalid());
    };
    let [path, is_dir, is_file, is_symlink] = items.as_slice() else {
        return Err(invalid());
    };
    let flag = |id: &NodeId| match graph.node(*id) {
        MontyNode::Bool(value) => Ok(*value),
        _ => Err(invalid()),
    };
    let (MontyNode::String(path) | MontyNode::Path(path)) = graph.node(*path) else {
        return Err(invalid());
    };
    let normalized = if path.starts_with('/') {
        None
    } else {
        normalize_relative(path)
    }
    .ok_or_else(|| format!("scan entry path {path:?} is not relative to the scan root"))?;
    Ok(ScanEntry {
        path: normalized.into_owned(),
        info: EntryInfo {
            is_dir: flag(is_dir)?,
            is_file: flag(is_file)?,
            is_symlink: flag(is_symlink)?,
        },
    })
}

impl PushValue for Vec<String> {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        let items = self.into_iter().map(|item| item.push_into(graph)).collect();
        graph.push(MontyNode::List(items))
    }
}

// =============================================================================
// Sources: what the selector reads a tree through.
// =============================================================================

/// A directory tree the glob selector and [`collect_scan`] read, with paths
/// relative to the scan root (`""` is the root).
///
/// Paths may carry `..` from a pattern's literal parts; implementations
/// normalize them and treat one that climbs above the root as absent.
pub trait ScanSource {
    /// A failure that aborts the scan, as opposed to an unreadable directory.
    type Error;

    /// Lists `dir`'s children, or `None` when it cannot be listed (missing,
    /// not a directory, unreadable) — CPython's glob skips those silently.
    fn list(&mut self, dir: &str) -> Result<Option<Vec<(String, EntryInfo)>>, Self::Error>;

    /// Describes `path` without following a final symlink (`os.path.lexists`),
    /// `None` when it does not exist.
    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, Self::Error>;

    /// Charged once per entry a scan examines, so a host can cap the work a
    /// pathological pattern (`*/**/*/**/...`) or a symlink cycle costs.
    fn visit(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Runs a scan against `source` for a host and returns every entry it read,
/// root included, or `None` when the root does not exist.
///
/// Listings are memoized, so each directory is read at most once however many
/// pattern states reach it. With a pattern only the entries [`GlobSelector`]
/// visits are read; without one, the tree to `max_depth`.
pub fn collect_scan<E>(args: &ScanArgs, source: &mut impl ScanSource<Error = E>) -> Result<Option<Vec<ScanEntry>>, E> {
    let mut recorder = Recorder {
        source,
        listed: HashMap::new(),
        entries: BTreeMap::new(),
    };
    let Some(root) = recorder.lookup("")? else {
        return Ok(None);
    };
    if root.is_dir {
        match &args.pattern {
            Some(parts) => {
                GlobSelector::new(parts, args.case_sensitive, args.recurse_symlinks).prune(&mut recorder)?;
            }
            None => walk_to_depth(&mut recorder, args.max_depth, args.follow_symlinks)?,
        }
    }
    Ok(Some(
        recorder
            .entries
            .into_iter()
            .map(|(path, info)| ScanEntry { path, info })
            .collect(),
    ))
}

/// Lists every directory shallower than `max_depth`, descending symlinked ones
/// only when `follow_symlinks`.
fn walk_to_depth<E>(
    source: &mut impl ScanSource<Error = E>,
    max_depth: Option<u32>,
    follow_symlinks: bool,
) -> Result<(), E> {
    let mut stack = vec![(String::new(), 0_u32)];
    while let Some((dir, depth)) = stack.pop() {
        if max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        for (name, info) in source.list(&dir)?.unwrap_or_default() {
            source.visit()?;
            if info.is_dir && (follow_symlinks || !info.is_symlink) {
                stack.push((join(&dir, &name), depth + 1));
            }
        }
    }
    Ok(())
}

/// Memoizes a host source's listings and records every entry they return.
struct Recorder<'s, S> {
    source: &'s mut S,
    /// Listings by normalized directory path.
    listed: HashMap<String, Option<Vec<(String, EntryInfo)>>>,
    /// Every entry seen, by normalized path; becomes the reply.
    entries: BTreeMap<String, EntryInfo>,
}

impl<E, S: ScanSource<Error = E>> ScanSource for Recorder<'_, S> {
    type Error = E;

    fn list(&mut self, dir: &str) -> Result<Option<Vec<(String, EntryInfo)>>, E> {
        let Some(dir) = normalize_relative(dir) else {
            return Ok(None);
        };
        if let Some(listing) = self.listed.get(dir.as_ref()) {
            return Ok(listing.clone());
        }
        // Record the directory itself too: a literal part reaches it without a
        // listing of its parent, and the sandbox only lists what it knows is a directory.
        if self.lookup(&dir)?.is_none_or(|info| !info.is_dir) {
            self.listed.insert(dir.into_owned(), None);
            return Ok(None);
        }
        let listing = self.source.list(&dir)?;
        for (name, info) in listing.iter().flatten() {
            self.entries.insert(join(&dir, name), *info);
        }
        self.listed.insert(dir.into_owned(), listing.clone());
        Ok(listing)
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, E> {
        let Some(path) = normalize_relative(path) else {
            return Ok(None);
        };
        if let Some(info) = self.entries.get(path.as_ref()) {
            return Ok(Some(*info));
        }
        let info = self.source.lookup(&path)?;
        if let Some(info) = info {
            self.entries.insert(path.into_owned(), info);
        }
        Ok(info)
    }

    fn visit(&mut self) -> Result<(), E> {
        self.source.visit()
    }
}

/// A scan reply indexed for the sandbox: the [`ScanSource`] its glob reads and
/// the per-directory listings its walks read.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanTree {
    /// Every entry by normalized relative path; the root is `""`.
    entries: HashMap<String, EntryInfo>,
    /// Children of each directory, sorted by name.
    children: HashMap<String, Vec<(String, EntryInfo)>>,
}

impl ScanTree {
    /// Indexes reply entries; a later duplicate of a path replaces the earlier one.
    #[must_use]
    pub fn new(entries: Vec<ScanEntry>) -> Self {
        let entries: HashMap<String, EntryInfo> = entries.into_iter().map(|entry| (entry.path, entry.info)).collect();
        let mut children: HashMap<String, Vec<(String, EntryInfo)>> = HashMap::new();
        for (path, info) in &entries {
            if !path.is_empty() {
                let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
                children
                    .entry(parent.to_owned())
                    .or_default()
                    .push((name.to_owned(), *info));
            }
        }
        for siblings in children.values_mut() {
            siblings.sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        Self { entries, children }
    }

    /// The root entry; a reply that omits it is taken to describe a directory.
    #[must_use]
    pub fn root(&self) -> EntryInfo {
        self.entries.get("").copied().unwrap_or(EntryInfo {
            is_dir: true,
            is_file: false,
            is_symlink: false,
        })
    }

    /// The root entry if the reply described it, unlike [`Self::root`].
    #[must_use]
    pub fn described_root(&self) -> Option<EntryInfo> {
        self.entries.get("").copied()
    }

    /// The sorted children of the normalized directory `dir`.
    #[must_use]
    pub fn children(&self, dir: &str) -> &[(String, EntryInfo)] {
        self.children.get(dir).map_or(&[], Vec::as_slice)
    }

    /// The entry at normalized path `path`.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<EntryInfo> {
        if path.is_empty() {
            Some(self.root())
        } else {
            self.entries.get(path).copied()
        }
    }
}

impl ScanSource for ScanTree {
    type Error = Infallible;

    fn list(&mut self, dir: &str) -> Result<Option<Vec<(String, EntryInfo)>>, Infallible> {
        let listing = normalize_relative(dir)
            .filter(|dir| self.get(dir).is_some_and(|info| info.is_dir))
            .map(|dir| self.children(&dir).to_vec());
        Ok(listing)
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, Infallible> {
        Ok(normalize_relative(path).and_then(|path| self.get(&path)))
    }
}

// =============================================================================
// The glob selector — a port of CPython's `glob._GlobberBase`.
// =============================================================================

/// Splits off the leading parts CPython resolves by string concatenation
/// rather than listing: `..`, and literals unless `case_sensitive` was given.
///
/// Returns the joined prefix and the rest. A prefix ending in `/` means the
/// pattern had a trailing slash, so the prefix must name a directory. Hoisting
/// the prefix into the scan root means the host lists nothing above it.
#[must_use]
pub fn split_literal_prefix(parts: &[String], case_sensitive: Option<bool>) -> (String, &[String]) {
    let mut prefix = String::new();
    let mut rest = parts;
    while let Some((part, tail)) = rest.split_first()
        && part != "**"
        && (is_special(part) || (case_sensitive.is_none() && !has_magic(part)))
    {
        if !prefix.is_empty() || part.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        rest = tail;
    }
    (prefix, rest)
}

/// Selects the paths a `Path.glob` pattern matches, relative to the scan root.
///
/// Mirrors CPython 3.14's selectors part for part — literal parts are looked up
/// rather than listed, `**` descends only real directories unless
/// `recurse_symlinks`, other wildcards descend symlinked ones — so running it
/// over a host's reply gives what CPython gives over the real tree. Unlike
/// CPython it works through an explicit stack and visits each pattern state
/// once, so neither recursion depth nor repeated states grow with the pattern.
#[derive(Debug)]
pub struct GlobSelector<'p> {
    /// Parsed pattern parts.
    parts: &'p [String],
    /// What each part index does, compiled once.
    steps: Vec<Step>,
    /// Whether `**` descends into symlinked directories.
    recurse_symlinks: bool,
}

/// What the selector does at one pattern part (see [`GlobSelector`]).
#[derive(Debug)]
enum Step {
    /// No parts left: yield the path if it exists.
    Exists,
    /// `''` or `..`: append it to the path without touching the filesystem.
    Special,
    /// Consecutive literal parts, joined with `/`, then continue at `next`.
    Literal { joined: String, next: usize },
    /// List the directory and keep children whose name matches.
    Wildcard {
        matcher: Option<SegmentPattern>,
        next: usize,
    },
    /// `**`: the path itself, then every descendant; `matcher` filters by the
    /// parts merged after it when symlinks are followed.
    Recursive { matcher: Option<MultiPattern>, next: usize },
}

impl<'p> GlobSelector<'p> {
    /// Compiles `parts` (see [`ScanArgs::pattern`] and [`ScanArgs::case_sensitive`]).
    #[must_use]
    pub fn new(parts: &'p [String], case_sensitive: Option<bool>, recurse_symlinks: bool) -> Self {
        let steps = (0..=parts.len())
            .map(|index| compile_step(parts, index, case_sensitive, recurse_symlinks))
            .collect();
        Self {
            parts,
            steps,
            recurse_symlinks,
        }
    }

    /// Selects every match, as paths relative to the scan root.
    ///
    /// A path keeps the spelling the pattern built (`sub/../a.txt`), and a
    /// directory matched with more parts to come keeps a trailing `/`, which
    /// the caller strips as `pathlib` does.
    pub fn select<E>(&self, source: &mut impl ScanSource<Error = E>) -> Result<Vec<String>, E> {
        let mut run = Run {
            stack: Vec::new(),
            out: Some(Vec::new()),
            normalize: false,
        };
        self.run(source, &mut run)?;
        Ok(run.out.unwrap_or_default())
    }

    /// Reads everything [`Self::select`] would read, without collecting matches.
    ///
    /// For hosts pruning a scan: states are normalized, so a pattern like
    /// `*/../*/../*` revisits one directory rather than spelling out every route
    /// to it, and nothing grows with the number of matches.
    pub fn prune<E>(&self, source: &mut impl ScanSource<Error = E>) -> Result<(), E> {
        let mut run = Run {
            stack: Vec::new(),
            out: None,
            normalize: true,
        };
        self.run(source, &mut run)
    }

    /// The selection loop behind [`Self::select`] and [`Self::prune`].
    fn run<E>(&self, source: &mut impl ScanSource<Error = E>, run: &mut Run) -> Result<(), E> {
        let mut seen = HashSet::new();
        run.push(0, String::new(), false);
        while let Some(state) = run.stack.pop() {
            if !seen.insert(state.clone()) {
                continue;
            }
            let (index, path, exists) = state;
            match &self.steps[index] {
                Step::Exists => {
                    if exists || lexists(&path, source)? {
                        run.emit(path);
                    }
                }
                Step::Special => {
                    let mut path = path + &self.parts[index];
                    if index + 1 < self.parts.len() {
                        path.push('/');
                    }
                    run.push(index + 1, path, exists);
                }
                Step::Literal { joined, next } => run.push(*next, path + joined, false),
                Step::Wildcard { matcher, next } => {
                    let dir_only = *next < self.parts.len();
                    for (name, info) in source.list(path.trim_end_matches('/'))?.unwrap_or_default() {
                        source.visit()?;
                        if matcher.as_ref().is_none_or(|matcher| matcher.matches(&name)) {
                            let entry = join(&path, &name);
                            if !dir_only {
                                run.emit(entry);
                            } else if info.is_dir {
                                run.push(*next, entry + "/", true);
                            }
                        }
                    }
                }
                Step::Recursive { matcher, next } => {
                    self.select_recursive(matcher.as_ref(), *next, path, exists, source, run)?;
                }
            }
        }
        Ok(())
    }

    /// Runs a `**` step: the path itself, then each descendant, depth first.
    fn select_recursive<E>(
        &self,
        matcher: Option<&MultiPattern>,
        next: usize,
        path: String,
        exists: bool,
        source: &mut impl ScanSource<Error = E>,
        run: &mut Run,
    ) -> Result<(), E> {
        let dir_only = next < self.parts.len();
        let match_pos = path.len();
        if matcher.is_none_or(|matcher| matcher.matches("")) {
            run.push(next, path.clone(), exists);
        }
        let mut dirs = vec![path];
        while let Some(dir) = dirs.pop() {
            for (name, info) in source.list(dir.trim_end_matches('/'))?.unwrap_or_default() {
                source.visit()?;
                let is_dir = info.is_dir && (self.recurse_symlinks || !info.is_symlink);
                if is_dir || !dir_only {
                    let entry = join(&dir, &name);
                    let matched = matcher.is_none_or(|matcher| matcher.matches(&entry[match_pos..]));
                    let entry = if dir_only { entry + "/" } else { entry };
                    if matched {
                        if dir_only {
                            run.push(next, entry.clone(), true);
                        } else {
                            run.emit(entry.clone());
                        }
                    }
                    if is_dir {
                        dirs.push(entry);
                    }
                }
            }
        }
        Ok(())
    }
}

/// The pending states and results of one [`GlobSelector`] run.
struct Run {
    /// States still to process: part index, path so far, known to exist.
    stack: Vec<(usize, String, bool)>,
    /// Matches, when collecting them.
    out: Option<Vec<String>>,
    /// Whether to normalize paths (keeping a trailing `/`) before queueing them.
    normalize: bool,
}

impl Run {
    /// Queues a state; a normalized path climbing above the root is dropped.
    fn push(&mut self, index: usize, path: String, exists: bool) {
        if !self.normalize {
            self.stack.push((index, path, exists));
        } else if let Some(normalized) = normalize_relative(path.trim_end_matches('/')) {
            let mut normalized = normalized.into_owned();
            if path.ends_with('/') && !normalized.is_empty() {
                normalized.push('/');
            }
            self.stack.push((index, normalized, exists));
        }
    }

    /// Records a match.
    fn emit(&mut self, path: String) {
        if let Some(out) = &mut self.out {
            out.push(path);
        }
    }
}

/// Decides what part `index` does, as CPython's `_GlobberBase.selector` does.
fn compile_step(parts: &[String], index: usize, case_sensitive: Option<bool>, recurse_symlinks: bool) -> Step {
    let Some(part) = parts.get(index) else {
        return Step::Exists;
    };
    // An explicit `case_sensitive` makes CPython list even literal parts
    // ("case pedantic"), since it cannot know the filesystem's own rule.
    let pedantic = case_sensitive.is_some();
    let case_sensitive = case_sensitive.unwrap_or(true);
    if part == "**" {
        // Consecutive `**` parts behave as one.
        let mut next = index + 1;
        while parts.get(next).is_some_and(|part| part == "**") {
            next += 1;
        }
        // Following symlinks, CPython folds the next non-special parts into one
        // regex matched against each descendant's path instead of listing again.
        let mut merged = vec![MultiPart::Recursive];
        if recurse_symlinks {
            while let Some(part) = parts.get(next).filter(|part| !is_special(part)) {
                merged.push(MultiPart::compile(part, case_sensitive));
                next += 1;
            }
        }
        let matcher = (merged.len() > 1).then_some(MultiPattern(merged));
        Step::Recursive { matcher, next }
    } else if is_special(part) {
        Step::Special
    } else if !pedantic && !has_magic(part) {
        let mut joined = part.clone();
        let mut next = index + 1;
        while let Some(part) = parts.get(next).filter(|part| !has_magic(part)) {
            joined.push('/');
            joined.push_str(part);
            next += 1;
        }
        if next < parts.len() {
            joined.push('/');
        }
        Step::Literal { joined, next }
    } else {
        let matcher = (part != "*").then(|| SegmentPattern::new(part, case_sensitive));
        Step::Wildcard {
            matcher,
            next: index + 1,
        }
    }
}

/// `os.path.lexists` on a selector path: a trailing `/` (or the root, which is
/// always reached as `root/`) requires a directory, following a symlink.
fn lexists<E>(path: &str, source: &mut impl ScanSource<Error = E>) -> Result<bool, E> {
    let info = source.lookup(path.trim_end_matches('/'))?;
    Ok(if path.is_empty() || path.ends_with('/') {
        info.is_some_and(|info| info.is_dir)
    } else {
        info.is_some()
    })
}

/// The parts CPython never matches against names: a trailing slash's `''`, `.` and `..`.
fn is_special(part: &str) -> bool {
    matches!(part, "" | "." | "..")
}

/// Whether a part contains a wildcard character, as `glob.magic_check`.
fn has_magic(part: &str) -> bool {
    part.contains(['*', '?', '['])
}

/// Joins a child name onto a relative directory path (`""` is the root).
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Lexically normalizes a relative path, `None` if `..` climbs above the root.
fn normalize_relative(path: &str) -> Option<Cow<'_, str>> {
    if path.is_empty() || path.split('/').all(|part| !is_special(part)) {
        Some(Cow::Borrowed(path))
    } else {
        let mut parts: Vec<&str> = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                part => parts.push(part),
            }
        }
        Some(Cow::Owned(parts.join("/")))
    }
}

// =============================================================================
// fnmatch-style matching.
// =============================================================================

/// One path segment's pattern with `fnmatch` semantics: `*`, `?`, `[...]`,
/// `[!...]`; a leading dot is matched like any other character.
///
/// Matching is linear-time backtracking over `*` only, never exponential, since
/// hosts run it on sandbox-chosen patterns.
#[derive(Debug, Clone)]
pub struct SegmentPattern {
    tokens: Vec<Token>,
    case_sensitive: bool,
}

/// A compiled `fnmatch` element.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A literal character.
    Char(char),
    /// `?`: any one character.
    Any,
    /// `*`: any run of characters (consecutive stars collapse).
    Star,
    /// `[...]`: one character in (or, negated, not in) the items.
    Class { negated: bool, items: Vec<(char, char)> },
}

impl SegmentPattern {
    /// Compiles a segment pattern the way `fnmatch._translate` reads it.
    #[must_use]
    pub fn new(pattern: &str, case_sensitive: bool) -> Self {
        let chars: Vec<char> = pattern.chars().collect();
        let mut tokens = Vec::new();
        let mut i = 0;
        while let Some(&c) = chars.get(i) {
            i += 1;
            match c {
                '*' => {
                    if tokens.last() != Some(&Token::Star) {
                        tokens.push(Token::Star);
                    }
                }
                '?' => tokens.push(Token::Any),
                '[' => match parse_class(&chars, i) {
                    Some((token, end)) => {
                        tokens.push(token);
                        i = end;
                    }
                    None => tokens.push(Token::Char('[')),
                },
                c => tokens.push(Token::Char(c)),
            }
        }
        Self { tokens, case_sensitive }
    }

    /// Whether `name` matches the whole pattern.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        let text: Vec<char> = name.chars().collect();
        let (mut p, mut t) = (0, 0);
        // The last `*` seen and the text position it is currently absorbing up to.
        let mut star: Option<(usize, usize)> = None;
        while t < text.len() {
            match self.tokens.get(p) {
                Some(Token::Star) => {
                    star = Some((p, t));
                    p += 1;
                }
                Some(token) if self.token_matches(token, text[t]) => {
                    p += 1;
                    t += 1;
                }
                _ => match star {
                    Some((star_p, star_t)) => {
                        p = star_p + 1;
                        t = star_t + 1;
                        star = Some((star_p, star_t + 1));
                    }
                    None => return false,
                },
            }
        }
        self.tokens[p..].iter().all(|token| *token == Token::Star)
    }

    /// Whether a single-character token accepts `c`.
    fn token_matches(&self, token: &Token, c: char) -> bool {
        let variants = if self.case_sensitive {
            [c, c, c]
        } else {
            [c, simple_lower(c), simple_upper(c)]
        };
        match token {
            Token::Char(expected) => variants
                .iter()
                .any(|v| *v == *expected || (!self.case_sensitive && simple_lower(*expected) == simple_lower(*v))),
            Token::Any => true,
            Token::Star => false,
            Token::Class { negated, items } => {
                let hit = variants
                    .iter()
                    .any(|v| items.iter().any(|(low, high)| (*low..=*high).contains(v)));
                hit != *negated
            }
        }
    }
}

/// Parses a `[...]` class whose body starts at `start`, returning the token and
/// the index after `]`, or `None` when unterminated (then `[` is literal).
///
/// A leading `!` negates and a `]` straight after `[` or `[!` is literal. Each
/// `a-b` is a range — reversed ranges match nothing — and a `-` that cannot
/// start one is literal, which reproduces `fnmatch`'s chunking.
fn parse_class(chars: &[char], start: usize) -> Option<(Token, usize)> {
    let mut end = start;
    if chars.get(end) == Some(&'!') {
        end += 1;
    }
    if chars.get(end) == Some(&']') {
        end += 1;
    }
    while chars.get(end).is_some_and(|c| *c != ']') {
        end += 1;
    }
    if end >= chars.len() {
        return None;
    }
    let (negated, body) = match &chars[start..end] {
        ['!', body @ ..] => (true, body),
        body => (false, body),
    };
    let mut items = Vec::new();
    let mut k = 0;
    while let Some(&c) = body.get(k) {
        if let (Some('-'), Some(&high)) = (body.get(k + 1), body.get(k + 2)) {
            items.push((c, high));
            k += 3;
        } else {
            items.push((c, c));
            k += 1;
        }
    }
    Some((Token::Class { negated, items }, end + 1))
}

/// Single-character lowercase, as `re.IGNORECASE` compares (multi-char folds keep `c`).
fn simple_lower(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(lower), None) => lower,
        _ => c,
    }
}

/// Single-character uppercase counterpart of [`simple_lower`].
fn simple_upper(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(upper), None) => upper,
        _ => c,
    }
}

/// Several parts matched against a `/`-separated path, as `glob.translate`
/// compiles the parts following `**` when symlinks are followed.
#[derive(Debug)]
struct MultiPattern(Vec<MultiPart>);

/// One part of a [`MultiPattern`].
#[derive(Debug)]
enum MultiPart {
    /// `**`: any number of segments — at least one when last, since the
    /// regex's trailing `.*` sits after a separator.
    Recursive,
    /// Exactly one segment matching the pattern.
    Segment(SegmentPattern),
}

impl MultiPart {
    /// Compiles one merged part.
    fn compile(part: &str, case_sensitive: bool) -> Self {
        if part == "**" {
            Self::Recursive
        } else {
            Self::Segment(SegmentPattern::new(part, case_sensitive))
        }
    }
}

impl MultiPattern {
    /// Whether `path` (relative, no trailing `/`) matches; dynamic programming
    /// over (part, segment), so linear in each.
    fn matches(&self, path: &str) -> bool {
        let segments: Vec<&str> = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/').collect()
        };
        let mut reachable = vec![false; segments.len() + 1];
        reachable[0] = true;
        for (index, part) in self.0.iter().enumerate() {
            let mut next = vec![false; segments.len() + 1];
            match part {
                MultiPart::Recursive => {
                    let at_least_one = index + 1 == self.0.len();
                    let mut any = false;
                    for (position, slot) in next.iter_mut().enumerate() {
                        if !at_least_one {
                            any |= reachable[position];
                        }
                        *slot = any;
                        if at_least_one {
                            any |= reachable[position];
                        }
                    }
                }
                MultiPart::Segment(pattern) => {
                    for (position, segment) in segments.iter().enumerate() {
                        if reachable[position] && pattern.matches(segment) {
                            next[position + 1] = true;
                        }
                    }
                }
            }
            reachable = next;
        }
        reachable[segments.len()]
    }
}
