use std::{cmp::Ordering, path::PathBuf};

use crate::{
    dir_walker::WalkData,
    utils::{
        is_filtered_out_due_to_file_time, is_filtered_out_due_to_invert_regex,
        is_filtered_out_due_to_regex,
    },
};

// (size, inode+device, (modified, accessed, changed)) as returned by
// platform::get_metadata; named so it can be shared between walker stages
pub type EntryMetadata = (u64, Option<(u64, u64)>, (i64, i64, i64));

#[derive(Debug, Eq, Clone)]
pub struct Node {
    pub name: PathBuf,
    pub size: u64,
    pub children: Vec<Node>,
    // Only ever assigned, never read: the walk dedups through its own
    // InodeSet, not through this field. Kept for future per-node identity
    // features (PERF-8 note: it used to be copied in the duplicate-name
    // rename path, which masked the dead_code lint).
    #[allow(dead_code)]
    pub inode_device: Option<(u64, u64)>,
    pub depth: usize,
    // PERF-3: the walker already knows this; storing it avoids a per-node
    // stat in -t extension aggregation and filter post-processing
    pub is_file: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FileTime {
    Modified,
    Accessed,
    Changed,
}

// BUG-7: timestamps keep their sign. `Node.size` stays u64 (shared with byte
// sizes / file counts), so filetime values are stored through an
// order-preserving bijection i64 -> u64 (sign-bit flip): u64 ordering then
// matches chronological ordering for max-aggregation and sorting, and
// display/JSON decode back to the true i64.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
pub fn encode_filetime(t: i64) -> u64 {
    (t as u64) ^ (1_u64 << 63)
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
pub fn decode_filetime(size: u64) -> i64 {
    (size ^ (1_u64 << 63)) as i64
}

impl From<crate::cli::FileTime> for FileTime {
    fn from(time: crate::cli::FileTime) -> Self {
        match time {
            crate::cli::FileTime::Modified => Self::Modified,
            crate::cli::FileTime::Accessed => Self::Accessed,
            crate::cli::FileTime::Changed => Self::Changed,
        }
    }
}

// PERF-2: metadata is fetched once per entry by the walker and handed in,
// instead of each stage (ignore checks, node building) stat-ing again
//
// PERF-5: `already_filtered` marks entries that already survived
// `ignore_file`'s is_file-gated checks (regex, invert regex, filetime) — for
// those the outcome here is necessarily "not filtered", so the size-zeroing
// checks are skipped instead of re-evaluating every regex on every file.
// Callers that did NOT go through those checks (directories from
// finalize_chain, non-regular entries) must pass false: the zeroing still
// applies to them (e.g. a directory whose own name matches the invert regex
// contributes no own-block size).
#[allow(clippy::too_many_arguments)]
pub fn build_node(
    dir: PathBuf,
    children: Vec<Node>,
    is_file: bool,
    already_filtered: bool,
    depth: usize,
    walk_data: &WalkData,
    metadata: Option<EntryMetadata>,
) -> Option<Node> {
    let by_filecount = walk_data.by_filecount;
    let by_filetime = &walk_data.by_filetime;

    metadata.map(|data| {
        let inode_device = data.1;

        let filtered_out = !already_filtered && {
            let regex_or_time_filtered = is_filtered_out_due_to_regex(walk_data.filter_regex, &dir)
                || is_filtered_out_due_to_invert_regex(walk_data.invert_filter_regex, &dir)
                || [
                    (&walk_data.filter_modified_time, data.2.0),
                    (&walk_data.filter_accessed_time, data.2.1),
                    (&walk_data.filter_changed_time, data.2.2),
                ]
                .iter()
                .any(|(filter_time, actual_time)| {
                    is_filtered_out_due_to_file_time(filter_time.as_ref(), *actual_time)
                });
            regex_or_time_filtered || (by_filecount && !is_file)
        };

        let size = if filtered_out {
            0
        } else if by_filecount {
            1
        } else if by_filetime.is_some() {
            match by_filetime {
                Some(FileTime::Modified) => encode_filetime(data.2.0),
                Some(FileTime::Accessed) => encode_filetime(data.2.1),
                Some(FileTime::Changed) => encode_filetime(data.2.2),
                None => unreachable!(),
            }
        } else {
            data.0
        };

        Node {
            name: dir,
            size,
            children,
            inode_device,
            depth,
            is_file,
        }
    })
}

// PERF-8: the ordering key is (size, name) only. The old impl recursed
// into children when size and name tied, making BinaryHeap operations
// worst-case O(subtree). That tie-break was worthless anyway: children
// order is nondeterministic under the parallel walk, so the deep compare
// gave an unstable order. eq mirrors the key to keep the Ord/Eq contract
// (cmp == Equal <=> eq) intact.
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size && self.name == other.name
    }
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        self.size
            .cmp(&other.size)
            .then_with(|| self.name.cmp(&other.name))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    // PERF-5: already_filtered=true must skip the regex zeroing (the caller
    // guarantees the file survived ignore_file), while directories and
    // unfiltered entries keep the size-zeroing semantics
    #[test]
    fn test_build_node_already_filtered_skips_regex_zeroing() {
        use regex::Regex;

        use crate::dir_walker::tests::create_walker;

        // -v keeps only matching files: a non-matching regex means the file
        // would be zeroed unless the caller already filtered it
        let regex = vec![Regex::new("zzz_nomatch").unwrap()];
        let mut walk_data = create_walker(true);
        walk_data.filter_regex = &regex;

        let file = PathBuf::from("/tmp/keepme.txt");
        let meta = Some((100, None, (0, 0, 0)));
        let kept = build_node(file.clone(), vec![], true, true, 1, &walk_data, meta).unwrap();
        assert_eq!(kept.size, 100);
        let zeroed = build_node(file, vec![], true, false, 1, &walk_data, meta).unwrap();
        assert_eq!(zeroed.size, 0);

        // A directory matching the invert regex is always zero-checked
        // (directories never take the already_filtered path)
        let invert = vec![Regex::new("matchdir").unwrap()];
        walk_data.filter_regex = &[];
        walk_data.invert_filter_regex = &invert;
        let dir = build_node(
            PathBuf::from("/tmp/matchdir"),
            vec![],
            false,
            false,
            0,
            &walk_data,
            meta,
        )
        .unwrap();
        assert_eq!(dir.size, 0);
    }

    // PERF-8: ordering key is (size, name); children never participate.
    // Same key with different subtrees must compare Equal (and eq must
    // agree, per the Ord/Eq contract) so heap operations stay O(1) here.
    #[test]
    fn test_node_ordering_key_is_size_and_name_only() {
        let mk = |name: &str, size: u64, child_sizes: Vec<u64>| Node {
            name: PathBuf::from(name),
            size,
            children: child_sizes
                .into_iter()
                .map(|s| Node {
                    name: PathBuf::from("c"),
                    size: s,
                    children: vec![],
                    inode_device: None,
                    depth: 1,
                    is_file: true,
                })
                .collect(),
            inode_device: None,
            depth: 0,
            is_file: false,
        };

        let a = mk("same", 10, vec![1, 2, 3]);
        let b = mk("same", 10, vec![9, 9]);
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_eq!(a, b);
        // both directions (antisymmetry of the key)
        assert_eq!(b.cmp(&a), Ordering::Equal);

        let bigger = mk("same", 11, vec![]);
        assert_eq!(a.cmp(&bigger), Ordering::Less);
        let name_later = mk("zzz", 10, vec![]);
        assert_eq!(a.cmp(&name_later), Ordering::Less);
    }

    #[test]
    fn test_filetime_encoding_preserves_order_and_sign() {
        // BUG-7 regression: unsigned_abs mirrored pre-1970 times around the
        // epoch; the sign-bit flip must be a bijection preserving i64 order
        assert_eq!(decode_filetime(encode_filetime(0)), 0);
        assert_eq!(decode_filetime(encode_filetime(i64::MIN)), i64::MIN);
        assert_eq!(decode_filetime(encode_filetime(i64::MAX)), i64::MAX);

        let pre1970 = chrono::Local
            .with_ymd_and_hms(1969, 6, 15, 0, 0, 0)
            .unwrap()
            .timestamp();
        let post1970 = 1_000_000_000;
        assert!(pre1970 < 0);
        assert!(encode_filetime(pre1970) < encode_filetime(0));
        assert!(encode_filetime(0) < encode_filetime(post1970));
        assert_eq!(decode_filetime(encode_filetime(pre1970)), pre1970);
    }
}
