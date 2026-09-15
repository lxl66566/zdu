use std::{
    cmp::Ordering,
    collections::HashSet,
    fs,
    fs::DirEntry,
    io::Error,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    },
};

use rayon::iter::{IntoParallelIterator, ParallelIterator};
use regex::Regex;

use crate::{
    node::{EntryMetadata, FileTime, Node, build_node},
    platform::{get_entry_metadata, get_metadata},
    progress::{ORDERING, Operation, PAtomicInfo, RuntimeErrors},
    utils::{
        is_filtered_out_due_to_file_time, is_filtered_out_due_to_invert_regex,
        is_filtered_out_due_to_regex,
    },
};

#[derive(Debug)]
pub enum Operator {
    Equal = 0,
    LessThan = 1,
    GreaterThan = 2,
}

// Walk options are boolean mode switches set once from the CLI
#[allow(clippy::struct_excessive_bools)]
pub struct WalkData<'a> {
    pub ignore_directories: HashSet<PathBuf>,
    pub filter_regex: &'a [Regex],
    pub invert_filter_regex: &'a [Regex],
    pub allowed_filesystems: HashSet<u64>,
    pub filter_modified_time: Option<(Operator, i64)>,
    pub filter_accessed_time: Option<(Operator, i64)>,
    pub filter_changed_time: Option<(Operator, i64)>,
    pub use_apparent_size: bool,
    pub by_filecount: bool,
    pub by_filetime: &'a Option<FileTime>,
    pub ignore_hidden: bool,
    pub follow_links: bool,
    pub progress_data: Arc<PAtomicInfo>,
    pub errors: Arc<Mutex<RuntimeErrors>>,
}

// How walk_dir decides whether a PendingDir's path is a directory. Carrying
// the classification (from data already in hand) avoids a fresh stat per
// directory: ~176k redundant statx on /nix/store (PERF-6).
#[derive(Debug, PartialEq)]
enum DirKind {
    /// readdir d_type already confirmed a real directory (not a symlink):
    /// walk unconditionally, no stat needed.
    TypedDir,
    /// Root path or a -L followed symlink: type is not known from d_type
    /// alone, walk_dir falls back to stat-based classification (once per
    /// root / once per followed link, so the cost is negligible).
    StatClassified,
}

// Per-directory bookkeeping used during the parallel walk. Each directory gets
// one `PendingDir`. Subdirectory tasks hold an `Arc` back to their parent so
// they can push their finished `Node` into the parent's `children` and
// decrement `pending`. When `pending` reaches zero the directory is ready to
// be built and handed up to its own parent.
struct PendingDir {
    dir: PathBuf,
    depth: usize,
    kind: DirKind,
    // PERF-2: fetched once when the entry is discovered (with follow-links
    // semantics), reused when the finished directory Node is built
    metadata: Option<EntryMetadata>,
    // Per-root visited-set for -L cycle detection; carried on the chain so
    // spawned subdirectory tasks inherit it without extra plumbing
    followed_dir_ids: Arc<Mutex<HashSet<(u64, u64)>>>,
    parent: Option<Arc<PendingDir>>,
    // Starts at 1 for the directory itself; incremented per spawned
    // subdirectory task. Each completion decrements by 1. Reaching 0
    // means this directory and all descendants are done.
    pending: AtomicUsize,
    children: Mutex<Vec<Node>>,
}

pub fn walk_it(dirs: HashSet<PathBuf>, walk_data: &WalkData) -> Vec<Node> {
    // PERF-4: roots are walked concurrently instead of one after another.
    // Cross-root hardlink dedup stays global (shared `inodes` below).
    let inodes: Mutex<HashSet<(u64, u64)>> = Mutex::new(HashSet::new());
    let top_level_nodes: Mutex<Vec<Node>> = Mutex::new(Vec::new());

    rayon::scope(|s| {
        for d in dirs {
            let inodes = &inodes;
            let top_level_nodes = &top_level_nodes;
            s.spawn(move |_| walk_root(d, walk_data, inodes, top_level_nodes));
        }
    });

    top_level_nodes.into_inner().unwrap()
}

fn walk_root(
    d: PathBuf,
    walk_data: &WalkData,
    inodes: &Mutex<HashSet<(u64, u64)>>,
    top_level_nodes: &Mutex<Vec<Node>>,
) {
    // Concurrent roots interleave their spinner path/counter updates; that is
    // cosmetic only (all counters are atomic).
    walk_data.progress_data.clear_state(&d);

    // Cycle-detection scope is per root: the same target reached via two
    // different roots should still be walked once per root. Distinct from the
    // hard-link inode dedup in `clean_inodes`: that drops duplicate Nodes
    // post-walk, this stops the walker from descending into the same
    // directory twice (junction/symlink cycles under -L).
    let followed_dir_ids = Arc::new(Mutex::new(HashSet::new()));

    let root_is_symlink = walk_data.follow_links
        && fs::symlink_metadata(&d).is_ok_and(|m| m.file_type().is_symlink());

    // Synthetic outer parent above the root. Lets `finalize_chain` build
    // the root's Node via the same code path as every other directory: it
    // pushes the finished root Node into `outer.children`, then bubbles
    // one more time and stops at outer's `parent: None` early-return
    // before any further build_node call. We drain `outer.children`
    // afterwards.
    let outer = Arc::new(PendingDir {
        dir: PathBuf::new(),
        depth: 0,
        // Never walked, only drained; value is irrelevant
        kind: DirKind::StatClassified,
        metadata: None,
        followed_dir_ids: followed_dir_ids.clone(),
        parent: None,
        pending: AtomicUsize::new(1),
        children: Mutex::new(Vec::new()),
    });
    // PERF-2: fetched once here; finalize_chain reuses it when the
    // finished root Node is built
    let root_metadata = get_metadata(
        &d,
        walk_data.use_apparent_size,
        walk_data.follow_links && root_is_symlink,
    );
    let root = Arc::new(PendingDir {
        dir: d,
        depth: 0,
        // Roots keep the stat-based classification: the lstat metadata
        // alone cannot express "symlink points to a directory" without -L,
        // which `Path::is_dir` currently resolves by following
        kind: DirKind::StatClassified,
        metadata: root_metadata,
        followed_dir_ids,
        parent: Some(outer.clone()),
        // Sentinel +1: ensures subdirectory tasks can't bubble through
        // finalize_chain until the root's own scan is done.
        pending: AtomicUsize::new(1),
        children: Mutex::new(Vec::new()),
    });

    // Single scope per root: all descendant work runs as flat tasks inside
    // it, so stack depth is O(1) regardless of tree depth. The visited-set
    // Arc rides on the PendingDir chain.
    rayon::scope(|s| {
        s.spawn(move |s| walk_dir(s, root, walk_data));
    });

    walk_data
        .progress_data
        .state
        .store(Operation::PREPARING, ORDERING);

    let mut outer_children = std::mem::take(&mut *outer.children.lock().unwrap());
    if let Some(node) = outer_children.pop()
        && let Some(cleaned) = clean_inodes(node, &mut inodes.lock().unwrap(), walk_data)
    {
        top_level_nodes.lock().unwrap().push(cleaned);
    }
}

// Remove files which have the same inode, we don't want to double count them.
fn clean_inodes(x: Node, inodes: &mut HashSet<(u64, u64)>, walk_data: &WalkData) -> Option<Node> {
    if !walk_data.use_apparent_size
        && let Some(id) = x.inode_device
        && !inodes.insert(id)
    {
        return None;
    }

    // Sort Nodes so iteration order is predictable
    let mut tmp: Vec<_> = x.children;
    tmp.sort_by(sort_by_inode);
    let new_children: Vec<_> = tmp
        .into_iter()
        .filter_map(|c| clean_inodes(c, inodes, walk_data))
        .collect();

    let actual_size = if walk_data.by_filetime.is_some() {
        // If by_filetime is Some, directory 'size' is the maximum filetime among child files
        // instead of disk size
        new_children
            .iter()
            .map(|c| c.size)
            .chain(std::iter::once(x.size))
            .max()
            .unwrap_or(0)
    } else {
        // If by_filetime is None, directory 'size' is the sum of disk sizes or file counts of child
        // files
        x.size + new_children.iter().map(|c| c.size).sum::<u64>()
    };

    Some(Node {
        name: x.name,
        size: actual_size,
        children: new_children,
        inode_device: x.inode_device,
        depth: x.depth,
        is_file: x.is_file,
    })
}

fn sort_by_inode(a: &Node, b: &Node) -> Ordering {
    // Sorting by inode is quicker than by sorting by name/size
    match (a.inode_device, b.inode_device) {
        (Some(x), Some(y)) => {
            if x.0 != y.0 {
                x.0.cmp(&y.0)
            } else if x.1 != y.1 {
                x.1.cmp(&y.1)
            } else {
                a.name.cmp(&b.name)
            }
        },
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => a.name.cmp(&b.name),
    }
}

// Check if `path` is inside ignored directory. `canonical_dir` is the
// canonicalized path of the directory containing `path` (computed once per
// directory): canonical(child) == canonical_dir + file name, so the absolute
// check below needs no per-entry canonicalize (PERF-1, ~2x slowdown on -X).
fn is_ignored_path(path: &Path, canonical_dir: Option<&Path>, walk_data: &WalkData) -> bool {
    if walk_data.ignore_directories.contains(path) {
        return true;
    }

    if !walk_data
        .ignore_directories
        .iter()
        .any(|ignored| ignored.is_absolute())
    {
        return false;
    }

    // Entry is inside an ignored absolute path; those are canonicalized in
    // main. If the parent's canonicalization failed, fall back to a
    // per-entry canonicalize.
    if let Some(dir) = canonical_dir {
        let file_name = path.file_name().unwrap_or_default();
        walk_data
            .ignore_directories
            .iter()
            .any(|ignored| ignored.is_absolute() && dir.join(file_name).starts_with(ignored))
    } else {
        let absolute_entry_path = fs::canonicalize(path).unwrap_or_default();
        walk_data
            .ignore_directories
            .iter()
            .any(|ignored| ignored.is_absolute() && absolute_entry_path.starts_with(ignored))
    }
}

// PERF-2: takes the metadata fetched once by process_entry instead of
// stat-ing per check; is_file comes from the DirEntry's file type (no stat)
fn ignore_file(
    path: &Path,
    is_file: bool,
    metadata: Option<&EntryMetadata>,
    canonical_dir: Option<&Path>,
    walk_data: &WalkData,
) -> bool {
    if is_ignored_path(path, canonical_dir, walk_data) {
        return true;
    }

    let is_dot_file = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'));

    if !walk_data.allowed_filesystems.is_empty()
        && let Some((_size, Some((_id, dev)), _gunk)) = metadata
        && !walk_data.allowed_filesystems.contains(dev)
    {
        return true;
    }
    if (walk_data.filter_accessed_time.is_some()
        || walk_data.filter_modified_time.is_some()
        || walk_data.filter_changed_time.is_some())
        && let Some((_, _, (modified_time, accessed_time, changed_time))) = metadata
        && is_file
        && [
            (&walk_data.filter_modified_time, *modified_time),
            (&walk_data.filter_accessed_time, *accessed_time),
            (&walk_data.filter_changed_time, *changed_time),
        ]
        .iter()
        .any(|(filter_time, actual_time)| {
            is_filtered_out_due_to_file_time(filter_time.as_ref(), *actual_time)
        })
    {
        return true;
    }

    // Keeping `walk_data.filter_regex.is_empty()` is important for performance reasons, it stops
    // unnecessary work
    if !walk_data.filter_regex.is_empty()
        && is_file
        && is_filtered_out_due_to_regex(walk_data.filter_regex, path)
    {
        return true;
    }

    if !walk_data.invert_filter_regex.is_empty()
        && is_file
        && is_filtered_out_due_to_invert_regex(walk_data.invert_filter_regex, path)
    {
        return true;
    }

    is_dot_file && walk_data.ignore_hidden
}

fn walk_dir<'scope>(
    scope: &rayon::Scope<'scope>,
    pending: Arc<PendingDir>,
    walk_data: &'scope WalkData<'scope>,
) {
    // PERF-6: classify from data in hand instead of a fresh `is_dir()` stat
    // per directory. Only roots and -L followed symlinks pay the stat: its
    // follow semantics decide walk vs file-node vs file_not_found error,
    // which d_type/lstat metadata alone cannot express.
    let is_dir = match pending.kind {
        DirKind::TypedDir => true,
        DirKind::StatClassified => pending.dir.is_dir(),
    };
    if is_dir {
        // Canonicalized once per directory, reused for every entry's ignore
        // check (PERF-1). Only needed when absolute ignore paths are in play.
        let canonical_dir: Option<PathBuf> = if walk_data
            .ignore_directories
            .iter()
            .any(|ignored| ignored.is_absolute())
        {
            fs::canonicalize(&pending.dir).ok()
        } else {
            None
        };
        // EINTR is the only retryable error. Looping iteratively (rather than
        // recursing on retry, like the old code) keeps stack depth O(1).
        // A directory gives up after MAX_EINTR_RETRIES retries: some
        // network/virtual filesystems return Interrupted forever, and without
        // a cap the walk would spin indefinitely (upstream v1.2.5 semantics).
        let mut eintr_retries = 0u32;
        loop {
            let entries = match fs::read_dir(&pending.dir) {
                Ok(entries) => entries,
                Err(ref failed) => {
                    record_error(failed, &pending.dir, walk_data);
                    if is_retryable(failed) {
                        eintr_retries += 1;
                        if eintr_retries > MAX_EINTR_RETRIES {
                            break;
                        }
                        continue;
                    }
                    break;
                },
            };

            // Drain into a Vec before doing anything observable on `pending`.
            // This is the load-bearing structural choice for retry safety: if
            // we decide to retry, we throw the Vec away and re-list, with no
            // spawned subdir tasks or pushed file nodes to roll back.
            let collected: Vec<_> = entries.collect();

            // If any entry yielded a retryable error, throw the Vec away and
            // re-list. We record only that one error (it bumps the shared
            // EINTR counter); other errors aren't recorded yet because they'll
            // resurface on retry if they're real, and recording them now
            // would log phantoms when the retry succeeds cleanly.
            if let Some(failed) = collected
                .iter()
                .filter_map(|r| r.as_ref().err())
                .find(|e| is_retryable(e))
            {
                record_error(failed, &pending.dir, walk_data);
                eintr_retries += 1;
                if eintr_retries > MAX_EINTR_RETRIES {
                    break;
                }
                continue;
            }

            // Commit point: from here on we mutate `pending`. File nodes
            // are accumulated thread-locally by rayon's collect (no lock
            // contention in the hot loop) and merged with one extend.
            // Subdirs spawn from inside process_entry and bubble their own
            // Node in via finalize_chain later; those still take the lock,
            // but at most once per subdir.
            //
            // Pre-reserve children capacity to `collected.len()` (an upper
            // bound: every entry contributes at most one child Node, either
            // as a file via the extend below or as a subdir via bubble-up
            // in finalize_chain).
            {
                let mut children = pending.children.lock().unwrap();
                children.reserve(collected.len());
            }

            let file_nodes: Vec<Node> = collected
                .into_par_iter()
                .filter_map(|r| match r {
                    Ok(entry) => {
                        process_entry(scope, &pending, &entry, canonical_dir.as_deref(), walk_data)
                    },
                    Err(failed) => {
                        record_error(&failed, &pending.dir, walk_data);
                        None
                    },
                })
                .collect();

            if !file_nodes.is_empty() {
                pending.children.lock().unwrap().extend(file_nodes);
            }
            break;
        }
    } else if !pending.dir.is_file() {
        let mut editable_error = walk_data.errors.lock().unwrap();
        let bad_file = pending.dir.as_os_str().to_string_lossy().into();
        editable_error.file_not_found.insert(bad_file);
    }

    finalize_chain(pending, walk_data);
}

// Returns the file's Node when the entry is a file (so the caller can
// gather it via rayon's collect). Returns None for ignored entries and
// for subdirectories. Subdirs spawn a walk task and contribute their
// Node later via finalize_chain instead.
fn process_entry<'scope>(
    scope: &rayon::Scope<'scope>,
    pending: &Arc<PendingDir>,
    entry: &DirEntry,
    canonical_dir: Option<&Path>,
    walk_data: &'scope WalkData<'scope>,
) -> Option<Node> {
    // PERF-2/5: one path allocation and one metadata fetch per entry, shared
    // by the ignore checks, the followed-link device/cycle checks and node
    // building (previously up to 2 opens + several stats per file).
    // Entry-based fetch: on Windows this reuses readdir data (no stat syscall)
    let path = entry.path();
    let file_type = entry.file_type().ok()?;
    let is_symlink = file_type.is_symlink();
    let is_file = file_type.is_file();
    let follow = walk_data.follow_links && is_symlink;
    let metadata = get_entry_metadata(entry, walk_data.use_apparent_size, follow);

    if ignore_file(&path, is_file, metadata.as_ref(), canonical_dir, walk_data) {
        return None;
    }

    // If the entry is a directory we'll spawn off a new task to walk it.
    if file_type.is_dir() || follow {
        if follow {
            // Resolve the followed link target's identity once. Used for:
            // 1. cycle detection: never descend into a filesystem object we already visited
            //    (Windows junction loops under -L counted the same subtree dozens of times)
            // 2. the -x device check: the cheap Windows metadata path returns no device for
            //    directories, so a junction to another volume would otherwise slip past
            //    `allowed_filesystems`
            let id = metadata.as_ref().and_then(|(_, id, _)| *id)?;
            if !walk_data.allowed_filesystems.is_empty()
                && !walk_data.allowed_filesystems.contains(&id.1)
            {
                return None;
            }
            if !pending.followed_dir_ids.lock().unwrap().insert(id) {
                return None;
            }
        }

        // Increment must happen before scope.spawn so a fast child's decrement
        // can never observe pending = 0 before this walk_dir's finalize_chain
        // runs. It can be Relaxed ordering because rayon's scope spawn does
        // its own fencing.
        pending.pending.fetch_add(1, AtomicOrdering::Relaxed);

        let child = Arc::new(PendingDir {
            dir: path,
            depth: pending.depth + 1,
            // Entries reaching this branch are d_type dirs or (only under
            // -L) followed symlinks whose target type is unknown until walk
            kind: if file_type.is_dir() {
                DirKind::TypedDir
            } else {
                DirKind::StatClassified
            },
            metadata,
            followed_dir_ids: pending.followed_dir_ids.clone(),
            parent: Some(pending.clone()),
            pending: AtomicUsize::new(1),
            children: Mutex::new(Vec::new()),
        });
        scope.spawn(move |s| walk_dir(s, child, walk_data));
        return None;
    }

    let node = build_node(path, vec![], is_file, pending.depth, walk_data, metadata);

    let prog_data = &walk_data.progress_data;
    prog_data.num_files.fetch_add(1, ORDERING);
    if let Some(ref n) = node
        && walk_data.by_filetime.is_none()
    {
        // Timestamps aren't bytes: skip them so the spinner total stays sane
        prog_data.total_file_size.fetch_add(n.size, ORDERING);
    }
    node
}

// Iteratively bubbles completions up the parent chain, taking exactly one
// lock per directory along the way.
//
// Each iteration "completes" `pending`. We carry `node_to_push` between
// iterations: it holds the previous level's built Node so we can push it
// into `pending.children` in the same critical section as our own
// decrement. That collapses what would otherwise be three separate locks
// per directory (push from child, decrement, take children) into one.
//
// Termination paths:
//   1. pending stays > 0 after decrement: not the last completer. Return with the prior level's
//      Node already pushed into our children.
//   2. pending hits 0 and parent is None: this is the synthetic outer created in `walk_it`. Its
//      `children` now holds the finished root Node; `walk_it` drains it after `rayon::scope`
//      returns.
fn finalize_chain(mut pending: Arc<PendingDir>, walk_data: &WalkData) {
    let mut node_to_push: Option<Node> = None;
    loop {
        // Single critical section per directory: push the prior level's
        // Node, decrement (atomically, since `pending` is a separate
        // primitive from the children Vec), and (if we're the last
        // completer) take our children so we can build our own Node
        // outside the lock.
        //
        // The fetch_sub runs while holding the children mutex. That's not
        // required for the atomic itself, but it lets the "am I last?"
        // check and the subsequent `take` happen back-to-back without
        // re-locking, and it serializes against any concurrent push from
        // a sibling task that hasn't yet decremented.
        let (parent, children) = {
            let mut children_guard = pending.children.lock().unwrap();
            if let Some(n) = node_to_push.take() {
                children_guard.push(n);
            }
            // Relaxed: the children mutex carries the happens-before edge
            // an Acquire fence would otherwise provide.
            if pending.pending.fetch_sub(1, AtomicOrdering::Relaxed) != 1 {
                return;
            }
            let Some(parent) = pending.parent.clone() else {
                return;
            };
            (parent, std::mem::take(&mut *children_guard))
        };
        node_to_push = build_node(
            pending.dir.clone(),
            children,
            false,
            pending.depth,
            walk_data,
            pending.metadata,
        );
        pending = parent;
    }
}

fn is_retryable(failed: &Error) -> bool {
    failed.kind() == std::io::ErrorKind::Interrupted
}

// Some network/virtual filesystems return Interrupted forever; without a cap
// the walk would spin indefinitely (upstream v1.2.5 gives up after 999)
const MAX_EINTR_RETRIES: u32 = 999;

fn record_error(failed: &Error, dir: &Path, walk_data: &WalkData) {
    let mut editable_error = walk_data.errors.lock().unwrap();
    match failed.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::InvalidInput => {
            editable_error
                .no_permissions
                .insert(dir.to_string_lossy().into());
        },
        std::io::ErrorKind::NotFound => {
            editable_error.file_not_found.insert(failed.to_string());
        },
        std::io::ErrorKind::Interrupted => {
            editable_error.interrupted_error += 1;
            // This does happen on some systems. It was set to 3 but sometimes zdu runs would exceed
            // this However, if there is no limit this results in infinite retrys and
            // zdu never finishes
            if editable_error.interrupted_error > 999 {
                eprintln!(
                    "Too many Interrupted Errors occurred while scanning filesystem, skipping: {}",
                    dir.to_string_lossy()
                );
            }
        },
        _ => {
            editable_error.unknown_error.insert(failed.to_string());
        },
    }
}

mod tests {

    #[allow(unused_imports)]
    use super::*;

    #[cfg(test)]
    fn create_node() -> Node {
        Node {
            name: PathBuf::new(),
            size: 10,
            children: vec![],
            inode_device: Some((5, 6)),
            depth: 0,
            is_file: true,
        }
    }

    #[cfg(test)]
    fn create_walker<'a>(use_apparent_size: bool) -> WalkData<'a> {
        use crate::PIndicator;
        let indicator = PIndicator::build_me();
        WalkData {
            ignore_directories: HashSet::new(),
            filter_regex: &[],
            invert_filter_regex: &[],
            allowed_filesystems: HashSet::new(),
            filter_modified_time: Some((Operator::GreaterThan, 0)),
            filter_accessed_time: Some((Operator::GreaterThan, 0)),
            filter_changed_time: Some((Operator::GreaterThan, 0)),
            use_apparent_size,
            by_filecount: false,
            by_filetime: &None,
            ignore_hidden: false,
            follow_links: false,
            progress_data: indicator.data.clone(),
            errors: Arc::new(Mutex::new(RuntimeErrors::default())),
        }
    }

    #[test]
    #[allow(clippy::redundant_clone)]
    fn test_should_ignore_file() {
        let mut inodes = HashSet::new();
        let n = create_node();
        let walkdata = create_walker(false);

        // First time we insert the node
        assert_eq!(
            clean_inodes(n.clone(), &mut inodes, &walkdata),
            Some(n.clone())
        );

        // Second time is a duplicate - we ignore it
        assert_eq!(clean_inodes(n.clone(), &mut inodes, &walkdata), None);
    }

    #[test]
    #[allow(clippy::redundant_clone)]
    fn test_should_not_ignore_files_if_using_apparent_size() {
        let mut inodes = HashSet::new();
        let n = create_node();
        let walkdata = create_walker(true);

        // If using apparent size we include Nodes, even if duplicate inodes
        assert_eq!(
            clean_inodes(n.clone(), &mut inodes, &walkdata),
            Some(n.clone())
        );
        assert_eq!(
            clean_inodes(n.clone(), &mut inodes, &walkdata),
            Some(n.clone())
        );
    }

    #[test]
    fn test_total_ordering_of_sort_by_inode() {
        use std::str::FromStr;

        let a = Node {
            name: PathBuf::from_str("a").unwrap(),
            size: 0,
            children: vec![],
            inode_device: Some((3, 66310)),
            depth: 0,
            is_file: false,
        };

        let b = Node {
            name: PathBuf::from_str("b").unwrap(),
            size: 0,
            children: vec![],
            inode_device: None,
            depth: 0,
            is_file: false,
        };

        let c = Node {
            name: PathBuf::from_str("c").unwrap(),
            size: 0,
            children: vec![],
            inode_device: Some((1, 66310)),
            depth: 0,
            is_file: false,
        };

        assert_eq!(sort_by_inode(&a, &b), Ordering::Greater);
        assert_eq!(sort_by_inode(&a, &c), Ordering::Greater);
        assert_eq!(sort_by_inode(&c, &b), Ordering::Greater);

        assert_eq!(sort_by_inode(&b, &a), Ordering::Less);
        assert_eq!(sort_by_inode(&c, &a), Ordering::Less);
        assert_eq!(sort_by_inode(&b, &c), Ordering::Less);
    }

    #[cfg(test)]
    fn count_nodes(node: &Node) -> usize {
        let mut count = 0;
        let mut stack: Vec<&Node> = vec![node];
        while let Some(n) = stack.pop() {
            count += 1;
            stack.extend(n.children.iter());
        }
        count
    }

    #[cfg(test)]
    fn max_depth(node: &Node) -> usize {
        let mut max = node.depth;
        let mut stack: Vec<&Node> = vec![node];
        while let Some(n) = stack.pop() {
            if n.depth > max {
                max = n.depth;
            }
            stack.extend(n.children.iter());
        }
        max
    }

    // Mac cannot handle long files names, with DEPTH levels the path becomes too long
    #[cfg_attr(target_os = "macos", ignore)]
    #[test]
    fn test_walk_deeply_nested_tree() {
        // Builds tmp/a/a/.../a (DEPTH levels) and walks it. Catches regressions
        // back to a recursive walker, which would risk stack overflow on deep
        // trees (the original motivation for the removed -S flag).
        const DEPTH: usize = 500;
        let tmp = tempfile::tempdir().unwrap();
        let mut path = tmp.path().to_path_buf();
        for _ in 0..DEPTH {
            path.push("a");
            fs::create_dir(&path).unwrap();
        }

        let walkdata = create_walker(true);
        let mut roots = HashSet::new();
        roots.insert(tmp.path().to_path_buf());

        let result = walk_it(roots, &walkdata);
        assert_eq!(result.len(), 1);
        assert_eq!(max_depth(&result[0]), DEPTH);
        // Root + DEPTH descendants, each holding exactly one child.
        assert_eq!(count_nodes(&result[0]), DEPTH + 1);
    }

    #[test]
    fn test_walk_wide_directory() {
        // Many sibling files in one directory exercise the per-directory
        // parallel iteration: every file pushes into the same parent's
        // `children` Mutex, and finalize_chain is the sole reader.
        use std::io::Write;
        const N: usize = 500;
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..N {
            let mut f = fs::File::create(tmp.path().join(format!("f{i}"))).unwrap();
            writeln!(f, "{i}").unwrap();
        }

        let walkdata = create_walker(true);
        let mut roots = HashSet::new();
        roots.insert(tmp.path().to_path_buf());

        let result = walk_it(roots, &walkdata);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].children.len(), N);
        assert_eq!(count_nodes(&result[0]), N + 1);
    }

    #[test]
    fn test_walk_multiple_roots_concurrently() {
        // PERF-4: roots walk in parallel; all must still be collected
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        fs::write(tmp1.path().join("a"), b"a").unwrap();
        fs::write(tmp2.path().join("b"), b"b").unwrap();

        let walkdata = create_walker(true);
        let mut roots = HashSet::new();
        roots.insert(tmp1.path().to_path_buf());
        roots.insert(tmp2.path().to_path_buf());

        let result = walk_it(roots, &walkdata);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_walk_missing_root_records_file_not_found() {
        // A root that is neither a dir nor a file hits the `else if
        // !pending.dir.is_file()` branch in walk_dir and should be recorded
        // under `file_not_found`.
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");

        let walkdata = create_walker(true);
        let mut roots = HashSet::new();
        roots.insert(missing.clone());

        let _ = walk_it(roots, &walkdata);
        let errors = walkdata.errors.lock().unwrap();
        assert!(
            errors
                .file_not_found
                .contains(&missing.to_string_lossy().into_owned()),
            "expected file_not_found to contain {missing:?}, got {:?}",
            errors.file_not_found
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_walk_permission_denied_subdir_is_recorded() {
        // A subdirectory we can't read should land in `no_permissions` via
        // record_error's PermissionDenied arm. Skipped when running as root,
        // since chmod 000 doesn't deny root.
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Probe: if we can still list it, we're effectively root (or the FS
        // ignores mode bits) and the test can't observe a PermissionDenied.
        if std::fs::read_dir(&locked).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let walkdata = create_walker(true);
        let mut roots = HashSet::new();
        roots.insert(tmp.path().to_path_buf());

        let _ = walk_it(roots, &walkdata);

        // Restore permissions before tempdir's Drop tries to clean up.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        let errors = walkdata.errors.lock().unwrap();
        assert!(
            errors
                .no_permissions
                .contains(&locked.to_string_lossy().into_owned()),
            "expected no_permissions to contain {locked:?}, got {:?}",
            errors.no_permissions
        );
    }
}
