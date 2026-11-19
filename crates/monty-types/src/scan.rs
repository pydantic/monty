//! Directory-tree scans: the one OS call behind `os.scandir`, `os.walk`,
//! `Path.walk`, `Path.glob` and `Path.rglob`.
//!
//! The sandbox cannot suspend part-way through an iterator, so it asks the host
//! for a whole bounded subtree in one [`OsFunctionCall::Scan`](crate::OsFunctionCall::Scan)
//! and walks or globs the reply itself.
//!
//! A glob pattern rides along so the host can prune. A host may reply with the
//! tree, which needs only *at least* the entries the sandbox needs: every entry
//! [`GlobSelector`] visits when run against the real tree. Extra entries are
//! harmless because the sandbox re-runs the selector over the reply, so a host
//! that ignores the pattern and returns everything within `max_depth` (descending
//! symlinks when `follow_symlinks` says so) is still correct. Or it may run the
//! selector itself and reply with the matches ([`ScanReply::Matches`]), which
//! the sandbox uses as they are. [`collect_scan`] is the implementation hosts
//! can share; it replies with matches unless they outnumber the entries read.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    convert::Infallible,
    rc::Rc,
    sync::Arc,
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

/// What a host answers a scan with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanReply {
    /// Entries for the sandbox to walk, list or glob itself.
    Tree(Vec<ScanEntry>),
    /// A glob's matches as [`GlobSelector::select`] returns them, already
    /// selected, so the sandbox skips selection; valid only for a glob scan.
    Matches(Vec<String>),
}

impl ScanReply {
    /// Encodes the reply: the tree as [`scan_reply`] does, matches as
    /// `{'matches': [path, ...]}`.
    #[must_use]
    pub fn into_object(self) -> MontyObject {
        match self {
            Self::Tree(entries) => scan_reply(entries),
            Self::Matches(matches) => MontyObject::dict([(
                MontyObject::string(MATCHES_KEY.to_owned()),
                MontyObject::list(matches.into_iter().map(MontyObject::string)),
            )]),
        }
    }
}

/// The key of a [`ScanReply::Matches`] reply's one-entry dict.
const MATCHES_KEY: &str = "matches";

/// Encodes a tree scan reply as the host returns it: a list of
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

/// Decodes a host's scan reply (see [`ScanReply::into_object`]); tree entries
/// may be tuples, lists or named tuples, and paths `str` or `Path`.
///
/// The error describes what was wrong for the sandbox's `RuntimeError`. Paths
/// are normalized, and one that is absolute or climbs above the root is refused.
pub fn parse_scan_reply(reply: &MontyObject) -> Result<ScanReply, String> {
    let (graph, _) = unstable::graph_parts(reply);
    match unstable::root_node(reply) {
        MontyNode::List(ids) => ids
            .iter()
            .map(|&id| parse_entry(graph, id))
            .collect::<Result<_, _>>()
            .map(ScanReply::Tree),
        MontyNode::Dict(pairs) => match matches_list(graph, pairs) {
            Some(ids) => ids
                .iter()
                .map(|&id| parse_match(graph, id))
                .collect::<Result<_, _>>()
                .map(ScanReply::Matches),
            None => Err(format!(
                "expected {{'{MATCHES_KEY}': [path, ...]}}, got a different dict"
            )),
        },
        _ => Err(format!(
            "expected a list of (path, is_dir, is_file, is_symlink) tuples, got {}",
            reply.as_ref().type_name()
        )),
    }
}

/// The match list of a `{'matches': [...]}` dict, `None` for any other dict.
fn matches_list<'g>(graph: &'g MontyGraph, pairs: &[(NodeId, NodeId)]) -> Option<&'g [NodeId]> {
    let [(key, value)] = pairs else {
        return None;
    };
    match (graph.node(*key), graph.node(*value)) {
        (MontyNode::String(key), MontyNode::List(ids)) if key == MATCHES_KEY => Some(ids),
        _ => None,
    }
}

/// Decodes one match: a relative path, normalized, keeping a trailing `/`.
fn parse_match(graph: &MontyGraph, id: NodeId) -> Result<String, String> {
    let (MontyNode::String(path) | MontyNode::Path(path)) = graph.node(id) else {
        return Err(format!(
            "expected glob matches to be paths, got {}",
            graph.type_name(id)
        ));
    };
    let trimmed = path.trim_end_matches('/');
    let normalized = if path.starts_with('/') {
        None
    } else {
        normalize_relative(trimmed)
    }
    .ok_or_else(|| format!("glob match {path:?} is not relative to the scan root"))?;
    let mut normalized = normalized.into_owned();
    if trimmed.len() < path.len() && !normalized.is_empty() {
        normalized.push('/');
    }
    Ok(normalized)
}

/// Bytes [`parse_scan_reply`] and [`ScanTree::new`] allocate for `reply`, roughly:
/// each path is held three times (parsed, keyed, listed) plus per-entry overhead.
/// A match is charged the same, which covers it being parsed and then spelled.
///
/// Lets the sandbox charge its memory limit before indexing a reply, without allocating.
#[must_use]
pub fn indexed_size(reply: &MontyObject) -> usize {
    /// Map slots, list slots and flags per entry, generously.
    const ENTRY_OVERHEAD: usize = 160;
    let (graph, _) = unstable::graph_parts(reply);
    let ids = match unstable::root_node(reply) {
        MontyNode::List(ids) => ids.as_slice(),
        MontyNode::Dict(pairs) => matches_list(graph, pairs).unwrap_or_default(),
        _ => return 0,
    };
    ids.iter()
        .map(|&id| {
            let path_len = match graph.node(id) {
                MontyNode::String(path) | MontyNode::Path(path) => path.len(),
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
    fn list(&mut self, dir: &str) -> Result<Option<Listing>, Self::Error>;

    /// Describes `path` without following a final symlink (`os.path.lexists`),
    /// `None` when it does not exist.
    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, Self::Error>;

    /// Called once per entry a scan examines, so a host can cap the work a
    /// pathological pattern (`*/**/*/**/...`) or a symlink cycle costs.
    ///
    /// No default: a host-side source, or a wrapper that fails to forward
    /// this, would leave that work unbounded without anything failing.
    fn visit(&mut self) -> Result<(), Self::Error>;

    /// Charged with the bytes of each glob state a scan holds, which a pattern
    /// can multiply per entry read (`*/../*/../*`), so a host can bound them
    /// with the entries.
    ///
    /// No default, for the same reason as [`Self::visit`].
    fn charge(&mut self, bytes: usize) -> Result<(), Self::Error>;

    /// Most matches [`GlobSelector::select`] collects from this source before
    /// it stops collecting them; unlimited by default.
    fn match_capacity(&self) -> usize {
        usize::MAX
    }
}

/// A directory's children by name, shared so memoized and indexed listings
/// are handed out without copying their names.
pub type Listing = Arc<[(String, EntryInfo)]>;

/// Runs a scan against `source` for a host and returns its reply, or `None`
/// when the root does not exist.
///
/// Listings are memoized, so each directory is read at most once however many
/// pattern states reach it. With a pattern only the entries [`GlobSelector`]
/// visits are read, and the reply is the matches, unless they outnumber those
/// entries (`**/*/**` yields an entry once per ancestor); then it is every
/// entry read, root included, as it is without a pattern for the tree to `max_depth`.
pub fn collect_scan<E>(args: &ScanArgs, source: &mut impl ScanSource<Error = E>) -> Result<Option<ScanReply>, E> {
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
                let selector = GlobSelector::new(parts, args.case_sensitive, args.recurse_symlinks);
                if let Some(matches) = selector.select(&mut recorder)? {
                    return Ok(Some(ScanReply::Matches(matches)));
                }
            }
            None => walk_to_depth(&mut recorder, args.max_depth, args.follow_symlinks)?,
        }
    }
    let entries = recorder
        .entries
        .into_iter()
        .map(|(path, info)| ScanEntry { path, info })
        .collect();
    Ok(Some(ScanReply::Tree(entries)))
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
        for (name, info) in source.list(&dir)?.unwrap_or_default().iter() {
            source.visit()?;
            if info.is_dir && (follow_symlinks || !info.is_symlink) {
                stack.push((join(&dir, name), depth + 1));
            }
        }
    }
    Ok(())
}

/// Memoizes a host source's listings, sorted by name as the sandbox's
/// [`ScanTree`] sorts them so matches come out in the same order, and records
/// every entry they return.
struct Recorder<'s, S> {
    source: &'s mut S,
    /// Listings by normalized directory path.
    listed: HashMap<String, Option<Listing>>,
    /// Every entry seen, by normalized path; becomes the reply.
    entries: BTreeMap<String, EntryInfo>,
}

impl<E, S: ScanSource<Error = E>> ScanSource for Recorder<'_, S> {
    type Error = E;

    fn list(&mut self, dir: &str) -> Result<Option<Listing>, E> {
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
        let mut listing = self.source.list(&dir)?;
        // Copied only if the source still shares an unsorted listing.
        if let Some(children) = &mut listing
            && !children.is_sorted_by(|(a, _), (b, _)| a <= b)
        {
            Arc::make_mut(children).sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        for (name, info) in listing.iter().flat_map(|children| children.iter()) {
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

    fn charge(&mut self, bytes: usize) -> Result<(), E> {
        self.source.charge(bytes)
    }

    /// Matches stay a smaller reply than the entries they are drawn from
    /// unless a pattern yields an entry more than once.
    fn match_capacity(&self) -> usize {
        self.entries.len()
    }
}

/// A scan reply indexed for the sandbox: the [`ScanSource`] its glob reads and
/// the per-directory listings its walks read.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanTree {
    /// Every entry by normalized relative path; the root is `""`.
    entries: HashMap<String, EntryInfo>,
    /// Children of each directory, sorted by name.
    children: HashMap<String, Listing>,
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
                let child = (name.to_owned(), *info);
                match children.get_mut(parent) {
                    Some(siblings) => siblings.push(child),
                    None => {
                        children.insert(parent.to_owned(), vec![child]);
                    }
                }
            }
        }
        let children = children
            .into_iter()
            .map(|(dir, mut siblings)| {
                siblings.sort_by(|(a, _), (b, _)| a.cmp(b));
                (dir, Listing::from(siblings))
            })
            .collect();
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
        self.children.get(dir).map_or(&[], |siblings| siblings)
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

    fn list(&mut self, dir: &str) -> Result<Option<Listing>, Infallible> {
        let listing = normalize_relative(dir)
            .filter(|dir| self.get(dir).is_some_and(|info| info.is_dir))
            .map(|dir| self.children.get(dir.as_ref()).cloned().unwrap_or_default());
        Ok(listing)
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, Infallible> {
        Ok(normalize_relative(path).and_then(|path| self.get(&path)))
    }

    /// Unbounded: a reply already received, whose reader applies its own limits.
    fn visit(&mut self) -> Result<(), Infallible> {
        Ok(())
    }

    /// Free, as [`Self::visit`] is.
    fn charge(&mut self, _bytes: usize) -> Result<(), Infallible> {
        Ok(())
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
/// CPython it works through an explicit stack over normalized paths and visits
/// each pattern state once, so the work is bounded by pattern parts × entries
/// × depth however the pattern is shaped (see [`Self::select`] for the cost).
#[derive(Debug)]
pub struct GlobSelector<'p> {
    /// Parsed pattern parts.
    parts: &'p [String],
    /// What each part index does, compiled once; `None` where no step leads.
    steps: Vec<Option<Step>>,
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
    ///
    /// Only the steps reachable from the first part are compiled. Each consumes
    /// the parts it covers, so compilation is linear in the pattern: compiling
    /// every index would copy a literal tail once per part it has.
    #[must_use]
    pub fn new(parts: &'p [String], case_sensitive: Option<bool>, recurse_symlinks: bool) -> Self {
        let mut steps: Vec<Option<Step>> = (0..=parts.len()).map(|_| None).collect();
        let mut index = 0;
        loop {
            let step = compile_step(parts, index, case_sensitive, recurse_symlinks);
            let next = match &step {
                Step::Exists => None,
                Step::Special => Some(index + 1),
                Step::Literal { next, .. } | Step::Wildcard { next, .. } | Step::Recursive { next, .. } => Some(*next),
            };
            steps[index] = Some(step);
            match next {
                Some(next) => index = next,
                None => break,
            }
        }
        Self {
            parts,
            steps,
            recurse_symlinks,
        }
    }

    /// Selects every match, as normalized paths relative to the scan root.
    ///
    /// A `..` after a wildcard is collapsed and its target matched once, where
    /// CPython spells out every route (`a/../b/../x`): keyed by spelling, the
    /// states would multiply by the fan-out per `..`. A directory matched with
    /// more parts to come keeps a trailing `/`, which the caller strips as
    /// `pathlib` does.
    ///
    /// `None` when the matches outgrew [`ScanSource::match_capacity`]: the run
    /// then stops collecting them but still reads everything it would have, so
    /// a host can reply with what it read instead.
    pub fn select<E>(&self, source: &mut impl ScanSource<Error = E>) -> Result<Option<Vec<String>>, E> {
        let mut run = Run {
            stack: Vec::new(),
            seen: HashSet::new(),
            out: Some(Vec::new()),
        };
        self.run(source, &mut run)?;
        Ok(run.out)
    }

    /// The selection loop behind [`Self::select`].
    fn run<E>(&self, source: &mut impl ScanSource<Error = E>, run: &mut Run) -> Result<(), E> {
        run.push(0, "", false, source)?;
        while let Some((index, path, exists)) = run.stack.pop() {
            let step = self.steps[index]
                .as_ref()
                .expect("states are only pushed by the step chain compiled from part 0");
            match step {
                Step::Exists => {
                    if exists || lexists(&path, source)? {
                        run.emit(path.to_string(), source);
                    }
                }
                Step::Special => {
                    let mut path = format!("{path}{}", self.parts[index]);
                    if index + 1 < self.parts.len() {
                        path.push('/');
                    }
                    run.push(index + 1, &path, exists, source)?;
                }
                Step::Literal { joined, next } => run.push(*next, &format!("{path}{joined}"), false, source)?,
                Step::Wildcard { matcher, next } => {
                    let dir_only = *next < self.parts.len();
                    for (name, info) in source.list(path.trim_end_matches('/'))?.unwrap_or_default().iter() {
                        source.visit()?;
                        if matcher.as_ref().is_none_or(|matcher| matcher.matches(name)) {
                            let entry = join(&path, name);
                            if !dir_only {
                                run.emit(entry, source);
                            } else if info.is_dir {
                                run.push(*next, &(entry + "/"), true, source)?;
                            }
                        }
                    }
                }
                Step::Recursive { matcher, next } => {
                    self.select_recursive(matcher.as_ref(), *next, &path, exists, source, run)?;
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
        path: &str,
        exists: bool,
        source: &mut impl ScanSource<Error = E>,
        run: &mut Run,
    ) -> Result<(), E> {
        let dir_only = next < self.parts.len();
        let match_pos = path.len();
        if matcher.is_none_or(|matcher| matcher.matches("")) {
            run.push(next, path, exists, source)?;
        }
        let mut dirs = vec![path.to_owned()];
        while let Some(dir) = dirs.pop() {
            for (name, info) in source.list(dir.trim_end_matches('/'))?.unwrap_or_default().iter() {
                source.visit()?;
                let is_dir = info.is_dir && (self.recurse_symlinks || !info.is_symlink);
                if is_dir || !dir_only {
                    let entry = join(&dir, name);
                    let matched = matcher.is_none_or(|matcher| matcher.matches(&entry[match_pos..]));
                    let entry = if dir_only { entry + "/" } else { entry };
                    if is_dir {
                        if matched {
                            if dir_only {
                                run.push(next, &entry, true, source)?;
                            } else {
                                run.emit(entry.clone(), source);
                            }
                        }
                        dirs.push(entry);
                    } else if matched {
                        // Only a file match gets here (`dir_only` is false), so it takes the path.
                        run.emit(entry, source);
                    }
                }
            }
        }
        Ok(())
    }
}

/// A glob state: part index, normalized path so far, known to exist. The path
/// is shared between the stack and the seen set.
type State = (usize, Rc<str>, bool);

/// Bytes a glob state holds beside its path: a slot on the stack and one in
/// the seen set, plus the path's reference counts.
const STATE_OVERHEAD: usize = 2 * size_of::<State>() + 2 * size_of::<usize>();

/// The pending states and results of one [`GlobSelector`] run.
struct Run {
    /// States still to process.
    stack: Vec<State>,
    /// Every state ever queued, so each is processed once.
    seen: HashSet<State>,
    /// Matches, until they outgrow the source's [`ScanSource::match_capacity`].
    out: Option<Vec<String>>,
}

impl Run {
    /// Queues a state with its path normalized (keeping a trailing `/`) unless
    /// it was queued before; one climbing above the root is dropped.
    ///
    /// Each new state is charged to `source`: it stays held in the seen set
    /// until the run ends, and a pattern like `*/../*/../*` holds one per
    /// entry for every wildcard part.
    fn push<E>(
        &mut self,
        index: usize,
        path: &str,
        exists: bool,
        source: &mut impl ScanSource<Error = E>,
    ) -> Result<(), E> {
        let trimmed_len = path.trim_end_matches('/').len();
        let dir = trimmed_len > 0 && trimmed_len < path.len();
        let normalized: Rc<str> = match normalize_relative(&path[..trimmed_len]) {
            None => return Ok(()),
            Some(Cow::Owned(mut normalized)) => {
                if dir && !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.into()
            }
            Some(Cow::Borrowed(_)) => path[..trimmed_len + usize::from(dir)].into(),
        };
        let state = (index, normalized, exists);
        if !self.seen.contains(&state) {
            source.charge(STATE_OVERHEAD + state.1.len())?;
            self.seen.insert(state.clone());
            self.stack.push(state);
        }
        Ok(())
    }

    /// Records a match, or stops collecting once `source` would hold one too many.
    fn emit<E>(&mut self, path: String, source: &impl ScanSource<Error = E>) {
        if let Some(out) = &mut self.out {
            if out.len() < source.match_capacity() {
                out.push(path);
            } else {
                self.out = None;
            }
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

/// Lexically normalizes a `/`-separated relative path, dropping `.` and empty
/// components and resolving `..`; `None` if `..` climbs above the root.
///
/// [`ScanTree`] stores and looks paths up in this form, so a caller spelling
/// one differently (`./sub`, `sub/../other`) normalizes it first.
#[must_use]
pub fn normalize_relative(path: &str) -> Option<Cow<'_, str>> {
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
/// Matching backtracks over `*` only, so it costs at most pattern length ×
/// name length (the regex CPython compiles the pattern to backtracks the same
/// way), never exponential. Mounts bound both lengths by their name limit.
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
        // Once a `[` finds no `]` ahead, no later `[` can either; skipping their
        // scans keeps a run of unterminated brackets linear.
        let mut unterminated = false;
        while let Some(&c) = chars.get(i) {
            i += 1;
            match c {
                '*' => {
                    if tokens.last() != Some(&Token::Star) {
                        tokens.push(Token::Star);
                    }
                }
                '?' => tokens.push(Token::Any),
                '[' if !unterminated => {
                    if let Some((token, end)) = parse_class(&chars, i) {
                        tokens.push(token);
                        i = end;
                    } else {
                        unterminated = true;
                        tokens.push(Token::Char('['));
                    }
                }
                c => tokens.push(Token::Char(c)),
            }
        }
        Self { tokens, case_sensitive }
    }

    /// Whether `name` matches the whole pattern.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        // `t` is a byte offset into `name`, always on a character boundary.
        let (mut p, mut t) = (0, 0);
        // The last `*` seen and the text position it is currently absorbing up to.
        let mut star: Option<(usize, usize)> = None;
        while let Some(c) = name[t..].chars().next() {
            match self.tokens.get(p) {
                Some(Token::Star) => {
                    star = Some((p, t));
                    p += 1;
                }
                Some(token) if self.token_matches(token, c) => {
                    p += 1;
                    t += c.len_utf8();
                }
                _ => match star {
                    Some((star_p, star_t)) => {
                        // The star was recorded before a character at `star_t`, so one is there.
                        let absorbed = name[star_t..].chars().next().map_or(1, char::len_utf8);
                        p = star_p + 1;
                        t = star_t + absorbed;
                        star = Some((star_p, t));
                    }
                    None => return false,
                },
            }
        }
        self.tokens[p..].iter().all(|token| *token == Token::Star)
    }

    /// Whether a single-character token accepts `c`.
    ///
    /// Case-insensitively, `re.IGNORECASE` compares lowercased characters and
    /// folds a class's ranges both ways, so the variants tried are `c`, its
    /// lowercase, that lowercase's uppercase (`K` → `k` → `K`, for `[A-Z]`) and
    /// its uppercase's lowercase (`ſ` → `S` → `s`, for `[a-z]`).
    fn token_matches(&self, token: &Token, c: char) -> bool {
        let variants = if self.case_sensitive {
            [c, c, c, c]
        } else {
            let lower = simple_lower(c);
            [c, lower, simple_upper(lower), simple_lower(simple_upper(c))]
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

/// Single-character lowercase, as `re.IGNORECASE` compares. Unicode's one
/// multi-character lowercase, `İ` → `i̇`, has the simple mapping `i`, its first
/// character, which is what CPython uses.
fn simple_lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
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
