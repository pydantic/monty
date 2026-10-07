//! `Path.scan` for mounts: the subtree behind `os.walk`, `os.scandir` and `Path.glob`.
//!
//! [`collect_scan`] does the traversal and pruning; [`MountSource`] answers its
//! listings and lookups by running the mount's own single-path requests, so
//! every path a scan touches goes through the same policy checks, mode rules
//! and descriptor confinement as a sandbox `Path.iterdir()` or `Path.is_dir()`
//! on it would.

use monty_types::{
    MontyObject, ScanArgs,
    scan::{EntryInfo, ScanSource, collect_scan, scan_reply},
    unstable::{self, MontyNode},
};

use super::{
    common::{LISTING_ENTRY_MEMORY_USAGE, MemoryBudget, MountContext, as_u64},
    dispatch::{self, FsRequest},
    error::MountError,
    mount_mode::MountMode,
    overlay::available_memory,
    path_security::reject_overlong_path,
};

/// Most entries one scan may examine, counting each time a pattern state
/// revisits a listing; bounds the work of patterns like `*/**/*/**/*`.
const MAX_SCAN_VISITS: u64 = 10_000_000;

/// Runs a scan against one mount, replying with the entries it read.
///
/// The reply is charged to the mount's memory limit as it grows. A missing root
/// raises `FileNotFoundError`; unreadable directories below it read as empty.
pub(super) fn execute(
    args: &ScanArgs,
    ctx: &mut MountContext<'_>,
    mode: &mut MountMode,
) -> Result<MontyObject, MountError> {
    if let Some(parts) = &args.pattern {
        reject_overlong_pattern(parts)?;
    }
    // Overlay data retained against the same limit leaves less for the reply.
    let budget = match &*mode {
        MountMode::OverlayMemory(state) => available_memory(state, ctx)?,
        MountMode::ReadWrite | MountMode::ReadOnly => MemoryBudget::full(ctx.memory_usage_limit),
    };
    let mut source = MountSource {
        root: args.path.trim_end_matches('/'),
        ctx,
        mode,
        budget,
        used: 0,
        visits: 0,
    };
    match collect_scan(args, &mut source)? {
        Some(entries) => Ok(scan_reply(entries)),
        None => Err(MountError::not_found(&args.path)),
    }
}

/// Applies the path length limits to a pattern without copying more of it
/// than it takes to exceed them, since a sandbox can send one of any size.
fn reject_overlong_pattern(parts: &[String]) -> Result<(), MountError> {
    /// One more component than paths may have, and one more byte than a name.
    const COMPONENTS: usize = 65;
    const NAME_BYTES: usize = 256;
    let mut joined = String::new();
    for part in parts.iter().take(COMPONENTS) {
        // Rounded up to a character boundary, so an over-long part stays over-long.
        let mut end = part.len().min(NAME_BYTES);
        while !part.is_char_boundary(end) {
            end += 1;
        }
        if !joined.is_empty() {
            joined.push('/');
        }
        joined.push_str(&part[..end]);
    }
    reject_overlong_path(&joined)
}

/// Reads one mount for [`collect_scan`] through its request handlers.
struct MountSource<'a, 'c> {
    /// Virtual path of the scan root, without a trailing `/`.
    root: &'a str,
    ctx: &'a mut MountContext<'c>,
    mode: &'a mut MountMode,
    /// The mount's memory limit, which the recorded entries are charged to.
    budget: MemoryBudget,
    /// Bytes charged so far.
    used: u64,
    /// Entries examined so far (see [`MAX_SCAN_VISITS`]).
    visits: u64,
}

impl MountSource<'_, '_> {
    /// Virtual path of `relative` below the scan root.
    fn virtual_path(&self, relative: &str) -> String {
        match (self.root, relative) {
            ("", "") => "/".to_owned(),
            (root, "") => root.to_owned(),
            ("", relative) => format!("/{relative}"),
            (root, relative) => format!("{root}/{relative}"),
        }
    }

    /// Runs one request, `None` for anything but a memory-limit failure: a
    /// lookup the mount refuses reads as absent, as it would to `Path.exists()`.
    fn request(&mut self, request: FsRequest) -> Result<Option<MontyObject>, MountError> {
        if reject_overlong_path(request.primary_path()).is_err() {
            return Ok(None);
        }
        match dispatch::execute(request, self.ctx, self.mode) {
            Ok(value) => Ok(Some(value)),
            Err(err @ MountError::MemoryUsageLimitExceeded(_)) => Err(err),
            Err(_) => Ok(None),
        }
    }

    /// Answers a boolean predicate request, `false` when it fails.
    fn predicate(&mut self, request: FsRequest) -> Result<bool, MountError> {
        Ok(self
            .request(request)?
            .is_some_and(|value| matches!(unstable::root_node(&value), MontyNode::Bool(true))))
    }

    /// Describes the entry at virtual path `path`.
    fn info(&mut self, path: &str) -> Result<EntryInfo, MountError> {
        Ok(EntryInfo {
            is_dir: self.predicate(FsRequest::IsDir { path: path.into() })?,
            is_file: self.predicate(FsRequest::IsFile { path: path.into() })?,
            is_symlink: self.predicate(FsRequest::IsSymlink { path: path.into() })?,
        })
    }

    /// Charges one recorded entry whose relative path is `path_len` bytes.
    fn charge(&mut self, path_len: usize) -> Result<(), MountError> {
        self.used = self
            .used
            .saturating_add(as_u64(path_len))
            .saturating_add(LISTING_ENTRY_MEMORY_USAGE);
        self.budget.check(self.used)
    }
}

impl ScanSource for MountSource<'_, '_> {
    type Error = MountError;

    fn list(&mut self, dir: &str) -> Result<Option<Vec<(String, EntryInfo)>>, MountError> {
        let dir_path = self.virtual_path(dir);
        let Some(listing) = self.request(FsRequest::Iterdir {
            path: dir_path.as_str().into(),
        })?
        else {
            return Ok(None);
        };
        let MontyNode::List(ids) = unstable::root_node(&listing) else {
            return Ok(None);
        };
        let (graph, _) = unstable::graph_parts(&listing);
        let mut children = Vec::with_capacity(ids.len());
        for id in ids {
            let (MontyNode::Path(child) | MontyNode::String(child)) = graph.node(*id) else {
                continue;
            };
            let name = child.rsplit_once('/').map_or(child.as_str(), |(_, name)| name);
            // The reply keeps the whole relative path, so that is what costs memory.
            self.charge(dir.len() + 1 + name.len())?;
            let info = self.info(&format!("{dir_path}/{name}"))?;
            children.push((name.to_owned(), info));
        }
        Ok(Some(children))
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, MountError> {
        let virtual_path = self.virtual_path(path);
        let info = self.info(&virtual_path)?;
        // Followed, so a link leaving the mount or dangling stays hidden, as it is from listings.
        let exists = self.predicate(FsRequest::Exists {
            path: virtual_path.as_str().into(),
        })?;
        if exists {
            self.charge(path.len())?;
        }
        Ok(exists.then_some(info))
    }

    fn visit(&mut self) -> Result<(), MountError> {
        self.visits += 1;
        if self.visits > MAX_SCAN_VISITS {
            Err(MountError::ScanLimitExceeded(MAX_SCAN_VISITS))
        } else {
            Ok(())
        }
    }
}
