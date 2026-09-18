#[allow(unused_imports)]
use std::fs;
use std::path::Path;

#[cfg(target_family = "unix")]
fn get_block_size() -> u64 {
    // All os specific implementations of MetadataExt seem to define a block as 512 bytes
    // https://doc.rust-lang.org/std/os/linux/fs/trait.MetadataExt.html#tymethod.st_blocks
    512
}

type InodeAndDevice = (u64, u64);
type FileTime = (i64, i64, i64);

#[cfg(target_family = "windows")]
// the resulting timestamp always fits in i64 by construction
#[allow(clippy::cast_possible_truncation)]
fn filetime_to_unix_seconds(filetime: u64) -> i64 {
    const TICKS_PER_SECOND: i128 = 10_000_000;
    const UNIX_EPOCH_FILETIME: i128 = 116_444_736_000_000_000;

    ((i128::from(filetime) - UNIX_EPOCH_FILETIME).div_euclid(TICKS_PER_SECOND)) as i64
}

#[cfg(target_family = "unix")]
pub fn get_metadata<P: AsRef<Path>>(
    path: P,
    use_apparent_size: bool,
    follow_links: bool,
) -> Option<(u64, Option<InodeAndDevice>, FileTime)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = if follow_links {
        path.as_ref().metadata()
    } else {
        path.as_ref().symlink_metadata()
    };
    match metadata {
        Ok(md) => {
            let file_size = md.len();
            if use_apparent_size {
                Some((
                    file_size,
                    Some((md.ino(), md.dev())),
                    (md.mtime(), md.atime(), md.ctime()),
                ))
            } else {
                // On NTFS mounts, the reported block count can be unexpectedly large.
                // To avoid overestimating disk usage, cap the allocated size to what the
                // file should occupy based on the file system I/O block size (blksize).
                // Related: https://github.com/bootandy/dust/issues/295
                // BUG-16: a few FUSE/virtual filesystems report
                // st_blksize == 0; div_ceil by zero panicked in a rayon
                // worker and poisoned the shared error-sink mutexes.
                let blksize = md.blksize().max(1);
                let target_size = file_size.div_ceil(blksize) * blksize;
                let reported_size = md.blocks() * get_block_size();

                // File systems can pre-allocate more space for a file than what would be necessary
                let pre_allocation_buffer = blksize * 65536;
                let max_size = target_size + pre_allocation_buffer;
                let allocated_size = if reported_size > max_size {
                    target_size
                } else {
                    reported_size
                };
                Some((
                    allocated_size,
                    Some((md.ino(), md.dev())),
                    (md.mtime(), md.atime(), md.ctime()),
                ))
            }
        },
        Err(_e) => None,
    }
}

// Shared Windows metadata gate: decides whether `md` (fetched either from a
// path stat or for free from directory enumeration) is trustworthy without
// opening the file handle.
#[cfg(target_family = "windows")]
fn metadata_from(
    md: &fs::Metadata,
    path: &Path,
    use_apparent_size: bool,
) -> Option<(u64, Option<InodeAndDevice>, FileTime)> {
    use std::os::windows::fs::MetadataExt;

    // On windows opening the file to get size, file ID and volume can be very
    // expensive because 1) it causes a few system calls, and more importantly 2) it can cause
    // windows defender to scan the file.
    // Therefore we try to avoid doing that for common cases, mainly those of
    // plain files:

    // The idea is to make do with the file size that we get from the OS for
    // free as part of iterating a folder. Therefore we want to make sure that
    // it makes sense to use that free size information:

    // Volume boundaries:
    // The user can ask us not to cross volume boundaries. If the DirEntry is a
    // plain file and not a reparse point or other non-trivial stuff, we assume
    // that the file is located on the same volume as the directory that
    // contains it.

    // File ID:
    // This optimization does deprive us of access to a file ID. As a
    // workaround, we just make one up that hopefully does not collide with real
    // file IDs.
    // Hard links: Unresolved. We don't get inode/file index, so hard links
    // count once for each link. Hopefully they are not too commonly in use on
    // windows.

    // Size:
    // We assume (naively?) that for the common cases the free size info is the
    // same as one would get by doing the expensive thing. Sparse and cloud
    // placeholder files are not included in the common cases, as one can
    // imagine there being more than one view on their size. Compressed and
    // encrypted files are: their stored size differs from the logical one,
    // but resolving it needs an open per file, which measurably dominates
    // the walk on NTFS-compressed trees (see NEEDS_EXPENSIVE_SIZE).

    // Savings in orders of magnitude in terms of time, io and cpu have been
    // observed on hdd, windows 10, some 100Ks files taking up some hundreds of
    // GBs:
    // Consistently opening the file: 30 minutes.
    // With this optimization:         8 sec.

    const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
    const FILE_ATTRIBUTE_READONLY: u32 = 0x01;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x02;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x04;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x0000_0200;
    const FILE_ATTRIBUTE_PINNED: u32 = 0x0008_0000;
    const FILE_ATTRIBUTE_UNPINNED: u32 = 0x0010_0000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    // Attributes for which the free logical size is unacceptably wrong in
    // allocated (default) mode, so those entries must take the expensive
    // path: sparse files (normally FILE_ATTRIBUTE_SPARSE_FILE would be
    // enough, however Windows sometimes likes to mask it out. see:
    // https://stackoverflow.com/q/54560454) and cloud placeholders.
    // FILE_ATTRIBUTE_COMPRESSED/ENCRYPTED are deliberately NOT here: their
    // stored size differs from logical too, but obtaining it needs an open
    // per file (~17us each, a measured 4x slowdown on a fully NTFS-compressed
    // tree); they report file length in allocated mode instead.
    const NEEDS_EXPENSIVE_SIZE: u32 = FILE_ATTRIBUTE_SPARSE_FILE
        | FILE_ATTRIBUTE_PINNED
        | FILE_ATTRIBUTE_UNPINNED
        | FILE_ATTRIBUTE_RECALL_ON_OPEN
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
        | FILE_ATTRIBUTE_OFFLINE;
    let attr_filtered = md.file_attributes()
        & !(FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_SYSTEM);
    let is_plain = (attr_filtered & FILE_ATTRIBUTE_ARCHIVE) != 0
        || (attr_filtered & FILE_ATTRIBUTE_DIRECTORY) != 0
        || md.file_attributes() == FILE_ATTRIBUTE_NORMAL;
    // In allocated (default) mode, entries whose on-disk size may differ from
    // the free logical size must take the expensive path. In apparent mode
    // the free size is already exact, so the plain-attribute check suffices.
    // BUG-3 (inherited from upstream dust): this gate used to key on
    // use_apparent_size the other way round, sending sparse/OneDrive files to
    // the expensive path only under -s, exactly where the free size suffices.
    if is_plain && (use_apparent_size || (attr_filtered & NEEDS_EXPENSIVE_SIZE) == 0) {
        Some((
            md.len(),
            None,
            (
                filetime_to_unix_seconds(md.last_write_time()),
                filetime_to_unix_seconds(md.last_access_time()),
                filetime_to_unix_seconds(md.creation_time()),
            ),
        ))
    } else {
        get_metadata_expensive(path, use_apparent_size)
    }
}

#[cfg(target_family = "windows")]
fn get_metadata_expensive(
    path: &Path,
    use_apparent_size: bool,
) -> Option<(u64, Option<InodeAndDevice>, FileTime)> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};

    use winapi_util::{Handle, file::information};

    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    // Opening with the full GENERIC_READ is the expensive part (defender
    // scans); FILE_READ_ATTRIBUTES alone is cheap
    // https://docs.microsoft.com/en-us/windows/win32/secauthz/generic-access-rights
    // FILE_FLAG_BACKUP_SEMANTICS is required to open directories (junction
    // targets included) and costs nothing extra; without it the expensive
    // path could never serve one (CreateFileW fails with access denied).
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    let h = Handle::from_file(file);
    let info = information(&h).ok()?;

    // BUG-15: winapi_util leaves the timestamps Option::None on some
    // network/virtual filesystems; unwrapping panicked inside a rayon worker
    // and poisoned every lock().unwrap() downstream. Fall back to 0 (= a
    // pre-epoch sentinel) instead of losing the whole entry.
    let times = (
        filetime_to_unix_seconds(info.last_write_time().unwrap_or(0)),
        filetime_to_unix_seconds(info.last_access_time().unwrap_or(0)),
        filetime_to_unix_seconds(info.creation_time().unwrap_or(0)),
    );
    let id = Some((info.file_index(), info.volume_serial_number()));

    // BUG-3 (inherited from upstream dust, which swaps the two kinds):
    // apparent size (-s) is the logical file size; the default (allocated)
    // mode is the on-disk size via GetCompressedFileSizeW, which is
    // sparse/compression aware and equals the logical size for plain files
    // (so it agrees with the fast path). There is no cheap way to obtain a
    // cluster-rounded allocation size without opening a handle, so Windows
    // "allocated" deliberately means "bytes actually stored", not blocks.
    if use_apparent_size {
        Some((info.file_size(), id, times))
    } else {
        use filesize::PathExt;
        Some((path.size_on_disk().ok()?, id, times))
    }
}

#[cfg(target_family = "windows")]
pub fn get_metadata<P: AsRef<Path>>(
    path: P,
    use_apparent_size: bool,
    follow_links: bool,
) -> Option<(u64, Option<InodeAndDevice>, FileTime)> {
    let path = path.as_ref();
    if follow_links {
        // Followed links need the target's file id for -L cycle detection and
        // the -x device filter; the fast path returns none. Without this the
        // walker's id lookup silently dropped every followed link whose
        // target was a plain file/dir (BUG-1). Symlinks are rare, so the
        // extra open per link is negligible.
        return get_metadata_expensive(path, use_apparent_size);
    }
    path.symlink_metadata()
        .ok()
        .and_then(|md| metadata_from(&md, path, use_apparent_size))
        .or_else(|| get_metadata_expensive(path, use_apparent_size))
}

// Entry-based variant: on Windows DirEntry::metadata() reuses the data from
// directory enumeration (no extra syscall), unlike a path stat. Falls back to
// the path-based path for followed links (target metadata differs from the
// link entry's own).
#[cfg(target_family = "windows")]
pub fn get_entry_metadata(
    entry: &fs::DirEntry,
    use_apparent_size: bool,
    follow_links: bool,
) -> Option<(u64, Option<InodeAndDevice>, FileTime)> {
    if follow_links {
        return get_metadata(entry.path(), use_apparent_size, true);
    }
    let path = entry.path();
    entry
        .metadata()
        .ok()
        .and_then(|md| metadata_from(&md, &path, use_apparent_size))
        .or_else(|| get_metadata_expensive(&path, use_apparent_size))
}

// Unix DirEntry::metadata() still issues a syscall; no gain over the path form
#[cfg(target_family = "unix")]
pub fn get_entry_metadata(
    entry: &fs::DirEntry,
    use_apparent_size: bool,
    follow_links: bool,
) -> Option<(u64, Option<InodeAndDevice>, FileTime)> {
    get_metadata(entry.path(), use_apparent_size, follow_links)
}

// Device of the filesystem containing `path`'s *target* (symlinks followed),
// for -x setup. Called once per root argument. GNU du -x stats its
// command-line arguments: the allowed volume is the target's, not the link's
// own (BUG-14: without -L the link's volume emptied the whole walk).
#[cfg(target_family = "unix")]
pub fn get_filesystem_device<P: AsRef<Path>>(path: P) -> Option<u64> {
    get_metadata(path, false, true).and_then(|(_, id, _)| id.map(|(_, dev)| dev))
}

// Same, but on Windows the cheap path returns no file id, which used to leave
// allowed_filesystems empty and -x silently inert (BUG-4): always take the
// expensive open, which also resolves reparse points. apparent mode skips
// the extra size_on_disk query; only the id is consumed.
#[cfg(target_family = "windows")]
pub fn get_filesystem_device<P: AsRef<Path>>(path: P) -> Option<u64> {
    get_metadata_expensive(path.as_ref(), true).and_then(|(_, id, _)| id.map(|(_, dev)| dev))
}
