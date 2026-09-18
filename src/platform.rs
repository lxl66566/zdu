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
                let blksize = md.blksize();
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
    // same as one would get by doing the expensive thing. Sparse, encrypted and
    // compressed files are not included in the common cases, as one can image
    // there being more than view on their size.

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
    // normally FILE_ATTRIBUTE_SPARSE_FILE would be enough, however Windows sometimes likes to mask it out. see: https://stackoverflow.com/q/54560454
    const IS_PROBABLY_ONEDRIVE: u32 = FILE_ATTRIBUTE_SPARSE_FILE
        | FILE_ATTRIBUTE_PINNED
        | FILE_ATTRIBUTE_UNPINNED
        | FILE_ATTRIBUTE_RECALL_ON_OPEN
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
        | FILE_ATTRIBUTE_OFFLINE;
    let attr_filtered = md.file_attributes()
        & !(FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_SYSTEM);
    if ((attr_filtered & FILE_ATTRIBUTE_ARCHIVE) != 0
        || (attr_filtered & FILE_ATTRIBUTE_DIRECTORY) != 0
        || md.file_attributes() == FILE_ATTRIBUTE_NORMAL)
        && !((attr_filtered & IS_PROBABLY_ONEDRIVE != 0) && use_apparent_size)
    {
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

    if use_apparent_size {
        use filesize::PathExt;
        Some((
            path.size_on_disk().ok()?,
            Some((info.file_index(), info.volume_serial_number())),
            (
                filetime_to_unix_seconds(info.last_write_time().unwrap()),
                filetime_to_unix_seconds(info.last_access_time().unwrap()),
                filetime_to_unix_seconds(info.creation_time().unwrap()),
            ),
        ))
    } else {
        Some((
            info.file_size(),
            Some((info.file_index(), info.volume_serial_number())),
            (
                filetime_to_unix_seconds(info.last_write_time().unwrap()),
                filetime_to_unix_seconds(info.last_access_time().unwrap()),
                filetime_to_unix_seconds(info.creation_time().unwrap()),
            ),
        ))
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
