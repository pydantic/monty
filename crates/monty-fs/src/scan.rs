//! `Path.scan` for mounts: the subtree behind `os.walk`, `os.scandir` and `Path.glob`.
//!
//! [`collect_scan`] does the traversal and pruning; [`MountSource`] answers its
//! listings and lookups through the mounts' own backends, each routed to the
//! mount its path selects, so every path a scan touches goes through the same
//! policy checks, mode rules and descriptor confinement as a sandbox
//! `Path.iterdir()` or `Path.is_dir()` on it would — a subtree mounted below
//! the scan root included. A listing types its entries from the directory
//! itself, so only symlinks and nested mount points cost a lookup each.

use monty_types::{
    MontyObject, ScanArgs,
    scan::{EntryInfo, ScanSource, collect_scan, scan_reply},
};

use super::{
    common::{LISTING_ENTRY_MEMORY_USAGE, MemoryBudget, PathInfo, as_u64},
    dispatch,
    error::MountError,
    mount_mode::MountMode,
    mount_table::{Mount, find_mount},
    overlay::available_memory,
    path_security::reject_overlong_path,
};

/// Most entries one scan may examine, counting each time a pattern state
/// revisits a listing; bounds the work of patterns like `*/**/*/**/*`.
const MAX_SCAN_VISITS: u64 = 10_000_000;

/// Runs a scan rooted in `mounts[root]`, replying with the entries it read.
///
/// The reply is charged to the root mount's memory limit as it grows. A
/// missing or unreadable root raises; unreadable directories below it read as
/// empty.
pub(super) fn execute(args: &ScanArgs, mounts: &mut [Mount], root: usize) -> Result<MontyObject, MountError> {
    if let Some(parts) = &args.pattern {
        reject_overlong_pattern(parts)?;
    }
    // Overlay data retained against the same limit leaves less for the reply.
    let budget = {
        let (ctx, mode) = mounts[root].backend();
        match &*mode {
            MountMode::OverlayMemory(state) => available_memory(state, &ctx)?,
            MountMode::ReadWrite | MountMode::ReadOnly => MemoryBudget::full(ctx.memory_usage_limit),
        }
    };
    let mut source = MountSource {
        root: args.path.trim_end_matches('/'),
        mounts,
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
///
/// Every part gets its separator, an empty one included, so the component
/// count seen is the pattern's own: emptied parts must not smuggle a long
/// tail past the limit and into the selector. A `/` inside a part (which the
/// interpreter never sends) splits it the same way, so no piece of what the
/// selector compiles escapes the name limit.
fn reject_overlong_pattern(parts: &[String]) -> Result<(), MountError> {
    /// One more component than paths may have, and one more byte than a name.
    const COMPONENTS: usize = 65;
    const NAME_BYTES: usize = 256;
    let mut joined = String::new();
    let segments = parts.iter().flat_map(|part| part.split('/')).take(COMPONENTS);
    for (index, segment) in segments.enumerate() {
        // Rounded up to a character boundary, so an over-long segment stays over-long.
        let mut end = segment.len().min(NAME_BYTES);
        while !segment.is_char_boundary(end) {
            end += 1;
        }
        if index > 0 {
            joined.push('/');
        }
        joined.push_str(&segment[..end]);
    }
    reject_overlong_path(&joined)
}

/// A directory's entries by name, each typed as a lookup of it would be.
type Listing = Vec<(String, EntryInfo)>;

/// Reads the mounts for [`collect_scan`] through their backends.
struct MountSource<'a> {
    /// Virtual path of the scan root, without a trailing `/`.
    root: &'a str,
    /// Every mount, longest prefix first; each request runs on the one its path selects.
    mounts: &'a mut [Mount],
    /// The root mount's memory limit, which the recorded entries are charged to.
    budget: MemoryBudget,
    /// Bytes charged so far.
    used: u64,
    /// Entries examined so far (see [`MAX_SCAN_VISITS`]).
    visits: u64,
}

impl MountSource<'_> {
    /// Virtual path of `relative` below the scan root.
    fn virtual_path(&self, relative: &str) -> String {
        match (self.root, relative) {
            ("", "") => "/".to_owned(),
            (root, "") => root.to_owned(),
            ("", relative) => format!("/{relative}"),
            (root, relative) => format!("{root}/{relative}"),
        }
    }

    /// Lists the directory at virtual path `path`, with the index of the mount
    /// that served it; `None` when the mount cannot. The root's failure is the
    /// scan's (CPython's `scandir` would raise it), one below reads as empty.
    fn listing(&mut self, dir: &str, path: &str) -> Result<Option<(usize, Listing)>, MountError> {
        if dir.is_empty() {
            return self.scan_list(path).map(Some);
        }
        if reject_overlong_path(path).is_err() {
            return Ok(None);
        }
        match self.scan_list(path) {
            Ok(listing) => Ok(Some(listing)),
            Err(err @ MountError::MemoryUsageLimitExceeded(_)) => Err(err),
            Err(_) => Ok(None),
        }
    }

    /// Lists `path` on the mount it selects, as the table would route an `iterdir`.
    fn scan_list(&mut self, path: &str) -> Result<(usize, Listing), MountError> {
        let Some(index) = find_mount(self.mounts, path) else {
            return Err(MountError::NoMountPoint(path.to_owned()));
        };
        let (ctx, mode) = self.mounts[index].backend();
        dispatch::scan_list(path, &ctx, mode).map(|entries| (index, entries))
    }

    /// Describes the entry at virtual path `path` as the four predicates would,
    /// in one lookup; a path the mount refuses is absent, as to `Path.exists()`.
    fn path_info(&mut self, path: &str) -> Result<PathInfo, MountError> {
        if reject_overlong_path(path).is_err() {
            return Ok(PathInfo::ABSENT);
        }
        let Some(index) = find_mount(self.mounts, path) else {
            return Ok(PathInfo::ABSENT);
        };
        let (ctx, mode) = self.mounts[index].backend();
        match dispatch::path_info(path, &ctx, mode) {
            Ok(info) => Ok(info),
            Err(err @ MountError::MemoryUsageLimitExceeded(_)) => Err(err),
            Err(_) => Ok(PathInfo::ABSENT),
        }
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

impl ScanSource for MountSource<'_> {
    type Error = MountError;

    fn list(&mut self, dir: &str) -> Result<Option<Vec<(String, EntryInfo)>>, MountError> {
        let dir_path = self.virtual_path(dir);
        let Some((index, listing)) = self.listing(dir, &dir_path)? else {
            return Ok(None);
        };
        let nested = self.mounts.len() > 1;
        let mut children = Vec::with_capacity(listing.len());
        for (name, info) in listing {
            // The reply keeps the whole relative path, so that is what costs memory.
            self.charge(dir.len() + 1 + name.len())?;
            let child = format!("{dir_path}/{name}");
            // A child another mount serves, or one too long to reach, is described
            // as a lookup of it would be rather than by its parent's listing.
            let info = if reject_overlong_path(&child).is_err()
                || (nested && find_mount(self.mounts, &child) != Some(index))
            {
                self.path_info(&child)?.info
            } else {
                info
            };
            children.push((name, info));
        }
        Ok(Some(children))
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, MountError> {
        // Followed, so a link leaving the mount or dangling stays hidden, as it is from listings.
        let PathInfo { info, exists } = self.path_info(&self.virtual_path(path))?;
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
