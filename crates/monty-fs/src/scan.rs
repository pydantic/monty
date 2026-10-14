//! `Path.scan` for mounts: the subtree behind `os.walk`, `os.scandir` and `Path.glob`.
//!
//! [`collect_scan`] does the traversal and pruning; [`MountSource`] answers its
//! listings and lookups through the mounts' own backends, each routed to the
//! mount its path selects, so every path a scan touches goes through the same
//! policy checks, mode rules and descriptor confinement as a sandbox
//! `Path.iterdir()` or `Path.is_dir()` on it would — a subtree mounted below
//! the scan root included. A listing types its entries from the directory
//! itself, so only symlinks and nested mount points cost a lookup each, and
//! opens its directory through its parent's handle ([`DirHandles`]) rather than
//! from the mount root.

use std::io::{self, ErrorKind};

use cap_std::fs::{Dir, Metadata, ReadDir};
use monty_types::{
    MontyObject, ScanArgs,
    scan::{EntryInfo, Listing, ScanSource, collect_scan},
};

use super::{
    common::{LISTING_ENTRY_MEMORY_USAGE, MemoryBudget, MountContext, PathInfo, as_u64, visible_entries},
    dispatch,
    error::MountError,
    mount_mode::MountMode,
    mount_table::{Mount, find_mount},
    overlay::{self, available_memory},
    path_security::{reject_drive_or_unc_segments, reject_overlong_path, resolve_virtual_path},
};

/// Most entries one scan may examine, counting each time a pattern state
/// revisits a listing; bounds the work of patterns like `*/**/*/**/*`.
const MAX_SCAN_VISITS: u64 = 10_000_000;

/// Runs a scan rooted in `mounts[root]`, replying with a glob's matches or the
/// entries it read (see [`collect_scan`]).
///
/// The entries read and a glob's pending and seen states are charged to the root mount's memory limit as they grow; matches,
/// which never outnumber the entries, are not charged again. A missing or
/// unreadable root raises; unreadable directories below it read as empty.
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
        handles: DirHandles::default(),
    };
    match collect_scan(args, &mut source)? {
        Some(reply) => Ok(reply.into_object()),
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
type Entries = Vec<(String, EntryInfo)>;

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
    /// Directories of direct mounts kept open to list their descendants through.
    handles: DirHandles,
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
    fn listing(&mut self, dir: &str, path: &str) -> Result<Option<(usize, Entries)>, MountError> {
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
    fn scan_list(&mut self, path: &str) -> Result<(usize, Entries), MountError> {
        let Some(index) = find_mount(self.mounts, path) else {
            return Err(MountError::NoMountPoint(path.to_owned()));
        };
        // The listing is built before its entries are charged, so it gets only what is left.
        let budget = self.budget.shrink(self.used)?;
        if let Some(listing) = self.list_through_handles(index, path, budget) {
            return Ok((index, listing));
        }
        let (ctx, mode) = self.mounts[index].backend();
        let listing = match &*mode {
            MountMode::OverlayMemory(state) => {
                let handles = &mut self.handles;
                overlay::scan_list(path, &ctx, state, budget, |rel| {
                    handles.read_dir_link_free(index, rel, ctx.mount_dir)
                })
            }
            MountMode::ReadWrite | MountMode::ReadOnly => dispatch::scan_list(path, &ctx, mode, budget),
        };
        listing.map(|entries| (index, entries))
    }

    /// Lists `path` on direct mount `index` through its parent's open handle.
    ///
    /// `None` for an overlay mount or any failure, `budget` exceeded included:
    /// [`Self::scan_list`] then lists the path from the mount root, which
    /// raises the error a listing would.
    fn list_through_handles(&mut self, index: usize, path: &str, budget: MemoryBudget) -> Option<Entries> {
        let (ctx, mode) = self.mounts[index].backend();
        if matches!(*mode, MountMode::OverlayMemory(_)) {
            return None;
        }
        let target = resolve_virtual_path(path, ctx.mount_virtual).ok()?;
        let rel = target.for_dir_op();
        let read_dir = self.handles.read_dir(index, rel, ctx.mount_dir)?;
        visible_entries(read_dir, ctx.mount_dir, rel, path, budget).ok()
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
        let handles = &mut self.handles;
        let info = match &*mode {
            MountMode::OverlayMemory(state) => overlay::path_info(path, &ctx, state, |rel| {
                handles.symlink_metadata(index, rel, ctx.mount_dir, true)
            }),
            MountMode::ReadWrite | MountMode::ReadOnly => match handles.direct_path_info(index, path, &ctx) {
                Some(info) => Ok(info),
                None => dispatch::path_info(path, &ctx, mode),
            },
        };
        match info {
            Ok(info) => Ok(info),
            Err(err @ MountError::MemoryUsageLimitExceeded(_)) => Err(err),
            Err(_) => Ok(PathInfo::ABSENT),
        }
    }

    /// Charges one recorded entry whose relative path is `path_len` bytes.
    fn charge_entry(&mut self, path_len: usize) -> Result<(), MountError> {
        self.charge_bytes(as_u64(path_len).saturating_add(LISTING_ENTRY_MEMORY_USAGE))
    }

    /// Adds `bytes` to what the scan holds, erroring past the budget.
    fn charge_bytes(&mut self, bytes: u64) -> Result<(), MountError> {
        self.used = self.used.saturating_add(bytes);
        self.budget.check(self.used)
    }
}

impl ScanSource for MountSource<'_> {
    type Error = MountError;

    fn list(&mut self, dir: &str) -> Result<Option<Listing>, MountError> {
        let dir_path = self.virtual_path(dir);
        let Some((index, mut entries)) = self.listing(dir, &dir_path)? else {
            return Ok(None);
        };
        let nested = self.mounts.len() > 1;
        // One buffer for every child's virtual path, rather than one per child.
        let mut child = String::new();
        for (name, info) in &mut entries {
            // The reply keeps the whole relative path, so that is what costs memory.
            self.charge_entry(dir.len() + 1 + name.len())?;
            child.clear();
            child.push_str(&dir_path);
            child.push('/');
            child.push_str(name);
            // A child another mount serves, or one path policy refuses (too long,
            // a drive or a `\` on Unix), is described as a lookup of it would be.
            if reject_overlong_path(&child).is_err()
                || reject_drive_or_unc_segments(name, &child).is_err()
                || (nested && find_mount(self.mounts, &child) != Some(index))
            {
                *info = self.path_info(&child)?.info;
            }
        }
        Ok(Some(entries.into()))
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, MountError> {
        // Followed, so a link leaving the mount or dangling stays hidden, as it is from listings.
        let PathInfo { info, exists } = self.path_info(&self.virtual_path(path))?;
        if exists {
            self.charge_entry(path.len())?;
        }
        Ok(exists.then_some(info))
    }

    /// Glob states share the entries' budget, so no pattern can hold more
    /// than the mount's limit.
    fn charge(&mut self, bytes: usize) -> Result<(), MountError> {
        self.charge_bytes(as_u64(bytes))
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

/// Directory handles one scan keeps open, so a directory is listed through its
/// parent's handle instead of by resolving every component from the mount root,
/// which cap-std does one `openat` at a time where the kernel cannot confine a
/// lookup itself (macOS).
///
/// Every handle descends from the mount's descriptor through cap-std, so what
/// is opened through it stays inside the mount. Only directories whose children
/// are listed are kept, at most [`Self::CAPACITY`], the least recently used going first.
#[derive(Default)]
struct DirHandles {
    /// `(mount index, mount-relative path, handle)`, most recently used last.
    open: Vec<(usize, String, Dir)>,
}

impl DirHandles {
    /// Most handles open at once; a depth-first scan revisits few ancestors.
    const CAPACITY: usize = 32;

    /// Lists mount `mount`'s directory at `rel` (`.` for its root) through its
    /// parent's handle; `root` is the mount's descriptor, the top-level parent.
    ///
    /// `None` when the parent cannot reach it, as with a symlink leaving the parent.
    fn read_dir(&mut self, mount: usize, rel: &str, root: &Dir) -> Option<ReadDir> {
        match rel.rsplit_once('/') {
            None => root.read_dir(rel).ok(),
            Some((parent, name)) => {
                let position = self.handle(mount, parent, root, false)?;
                self.open[position].2.read_dir(name).ok()
            }
        }
    }

    /// Lists overlay mount `mount`'s directory at `rel` through handles opened
    /// only on directories that are not symlinks, as the overlay's own
    /// component walk requires; `None` when one is a link, or anything fails.
    fn read_dir_link_free(&mut self, mount: usize, rel: &str, root: &Dir) -> Option<ReadDir> {
        if rel == "." {
            return root.entries().ok();
        }
        let position = self.handle(mount, rel, root, true)?;
        self.open[position].2.entries().ok()
    }

    /// Describes `path` on a direct mount with one `lstat` through its parent's
    /// handle, as [`PathInfo::from_host`] would.
    ///
    /// `None` when that cannot settle it: a symlink, whose target is resolved
    /// from the mount root, or any failure but a missing name.
    fn direct_path_info(&mut self, mount: usize, path: &str, ctx: &MountContext<'_>) -> Option<PathInfo> {
        let target = resolve_virtual_path(path, ctx.mount_virtual).ok()?;
        match self.symlink_metadata(mount, target.for_dir_op(), ctx.mount_dir, false)? {
            Ok(meta) if !meta.is_symlink() => Some(PathInfo {
                info: EntryInfo {
                    is_dir: meta.is_dir(),
                    is_file: meta.is_file(),
                    is_symlink: false,
                },
                exists: true,
            }),
            Err(err) if err.kind() == ErrorKind::NotFound => Some(PathInfo::ABSENT),
            _ => None,
        }
    }

    /// `lstat`s mount `mount`'s entry at `rel` through its parent's handle (see
    /// [`Self::handle`] for `refuse_links`); `None` for the mount root, or when
    /// the parent cannot be opened.
    fn symlink_metadata(
        &mut self,
        mount: usize,
        rel: &str,
        root: &Dir,
        refuse_links: bool,
    ) -> Option<io::Result<Metadata>> {
        match rel.rsplit_once('/') {
            None if rel == "." => None,
            None => Some(root.symlink_metadata(rel)),
            Some((parent, name)) => {
                let position = self.handle(mount, parent, root, refuse_links)?;
                Some(self.open[position].2.symlink_metadata(name))
            }
        }
    }

    /// Where the handle of `rel` is kept, opening it below its parent's handle,
    /// recursively, if it is not open.
    ///
    /// With `refuse_links`, a component is opened only if `lstat` finds a real
    /// directory, as the overlay's component walk does; a mount always passes
    /// the same value, so its handles all follow one rule.
    fn handle(&mut self, mount: usize, rel: &str, root: &Dir, refuse_links: bool) -> Option<usize> {
        if let Some(position) = self
            .open
            .iter()
            .position(|(index, path, _)| *index == mount && path == rel)
        {
            let entry = self.open.remove(position);
            self.open.push(entry);
        } else {
            let (parent, name) = match rel.rsplit_once('/') {
                None => (root, rel),
                Some((parent, name)) => {
                    let position = self.handle(mount, parent, root, refuse_links)?;
                    (&self.open[position].2, name)
                }
            };
            if refuse_links && !parent.symlink_metadata(name).is_ok_and(|meta| meta.is_dir()) {
                return None;
            }
            let dir = parent.open_dir(name).ok()?;
            if self.open.len() == Self::CAPACITY {
                self.open.remove(0);
            }
            self.open.push((mount, rel.to_owned(), dir));
        }
        Some(self.open.len() - 1)
    }
}
