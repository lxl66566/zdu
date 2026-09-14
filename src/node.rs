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
#[allow(clippy::too_many_arguments)]
pub fn build_node(
    dir: PathBuf,
    children: Vec<Node>,
    is_file: bool,
    depth: usize,
    walk_data: &WalkData,
    metadata: Option<EntryMetadata>,
) -> Option<Node> {
    let by_filecount = walk_data.by_filecount;
    let by_filetime = &walk_data.by_filetime;

    metadata.map(|data| {
        let inode_device = data.1;

        let size = if is_filtered_out_due_to_regex(walk_data.filter_regex, &dir)
            || is_filtered_out_due_to_invert_regex(walk_data.invert_filter_regex, &dir)
            || by_filecount && !is_file
            || [
                (&walk_data.filter_modified_time, data.2.0),
                (&walk_data.filter_accessed_time, data.2.1),
                (&walk_data.filter_changed_time, data.2.2),
            ]
            .iter()
            .any(|(filter_time, actual_time)| {
                is_filtered_out_due_to_file_time(filter_time.as_ref(), *actual_time)
            }) {
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

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.size == other.size && self.children == other.children
    }
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        self.size
            .cmp(&other.size)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.children.cmp(&other.children))
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
