use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use regex::Regex;

use crate::{config::DAY_SECONDS, dir_walker::Operator, platform};

pub fn simplify_dir_names<P: AsRef<Path>>(dirs: &[P]) -> HashSet<PathBuf> {
    // PERF-1: the previous pairwise is_a_parent_of scan was O(n^2) — seconds
    // of startup for 20k+ --files-from entries. Sort by components instead:
    // a path and its whole subtree form one contiguous block in that order
    // (every sequence between a prefix and its extensions must itself carry
    // the prefix), so a single starts_with check against the last kept path
    // removes every descendant, and the parent is kept.
    let mut normalized: Vec<PathBuf> = dirs.iter().map(normalize_path).collect();
    // Component order, not byte order: with byte order "a/b!x" ('!' < '/')
    // would sort between "a/b" and its real children and break the scan.
    // On Windows the per-component order additionally folds ASCII case
    // (BUG-2): byte order would sort "Z:/x" and "z:/x" apart with unrelated
    // paths in between, and the linear scan would keep both spellings.
    #[cfg(target_os = "windows")]
    normalized.sort_by(|a, b| cmp_components_ignore_case(a.components(), b.components()));
    #[cfg(not(target_os = "windows"))]
    normalized.sort_by(|a, b| a.components().cmp(b.components()));

    let mut top_level_names = HashSet::with_capacity(normalized.len());
    let mut last_kept: Option<&Path> = None;
    for path in &normalized {
        // Equal duplicates and case-variant spellings (Windows) also
        // start_with the kept path and are dropped
        let is_descendant = matches!(last_kept, Some(kept) if path_starts_with(kept, path));
        if !is_descendant {
            last_kept = Some(path.as_path());
            top_level_names.insert(path.clone());
        }
    }
    top_level_names
}

// Volume ids of the filesystems the root arguments live on, for -x.
// Arguments are always resolved with follow semantics (like GNU du -x):
// the allowed set must contain each root's *target* volume, otherwise a
// symlink argument without -L contributes the link's own volume and the
// target's contents get filtered to nothing (BUG-14).
pub fn get_filesystem_devices<P: AsRef<Path>>(paths: &[P]) -> HashSet<u64> {
    paths
        .iter()
        .filter_map(platform::get_filesystem_device)
        .collect()
}

pub fn normalize_path<P: AsRef<Path>>(path: P) -> PathBuf {
    // normalize path ...
    // 1. removing repeated separators
    // 2. removing interior '.' ("current directory") path segments
    // 3. removing trailing extra separators and '.' ("current directory") path segments
    // * `Path.components()` does all the above work; ref: <https://doc.rust-lang.org/std/path/struct.Path.html#method.components>
    // 4. changing to os preferred separator (automatically done by recollecting components back
    //    into a PathBuf)
    path.as_ref().components().collect()
}

// Canonicalize the path only if it is an absolute path
pub fn canonicalize_absolute_path(path: PathBuf) -> PathBuf {
    if !path.is_absolute() {
        return path;
    }
    match std::fs::canonicalize(&path) {
        Ok(canonicalized_path) => canonicalized_path,
        Err(_) => path,
    }
}

pub fn is_filtered_out_due_to_regex(filter_regex: &[Regex], dir: &Path) -> bool {
    if filter_regex.is_empty() {
        false
    } else {
        filter_regex
            .iter()
            .all(|f| !f.is_match(&dir.as_os_str().to_string_lossy()))
    }
}

pub fn is_filtered_out_due_to_file_time(
    filter_time: Option<&(Operator, i64)>,
    actual_time: i64,
) -> bool {
    match filter_time {
        None => false,
        Some((Operator::Equal, bound_time)) => {
            !(actual_time >= *bound_time && actual_time < *bound_time + DAY_SECONDS)
        },
        Some((Operator::GreaterThan, bound_time)) => actual_time < *bound_time,
        Some((Operator::LessThan, bound_time)) => actual_time > *bound_time,
    }
}

pub fn is_filtered_out_due_to_invert_regex(filter_regex: &[Regex], dir: &Path) -> bool {
    filter_regex
        .iter()
        .any(|f| f.is_match(&dir.as_os_str().to_string_lossy()))
}

// BUG-2: Windows filesystems (NTFS/FAT) match names case-insensitively, so
// path comparisons used for root dedup, -X ignore matching and --collapse
// must fold case per component: `z:/Temp/x` and `Z:/TEMP/X` are the same
// tree. Folding is ASCII-only (full Unicode upcase needs OS tables and the
// CLI surface is overwhelmingly ASCII). Unix paths stay byte-exact.

// Component-wise prefix test: true when `parent`'s components are a prefix
// of `child`'s. Components has no len() (not ExactSizeIterator), so the
// exhaustion pattern below replaces the count check std's starts_with does.
pub fn path_starts_with(parent: &Path, child: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        let mut parent_components = parent.components();
        let mut child_components = child.components();
        loop {
            match (parent_components.next(), child_components.next()) {
                // parent exhausted: it is a prefix of child
                (None, _) => return true,
                // parent still has components but child is exhausted
                (Some(_), None) => return false,
                (Some(p), Some(c)) => {
                    if !component_eq_ignore_case(&p, &c) {
                        return false;
                    }
                },
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        child.starts_with(parent)
    }
}

// Component-wise equality with the same case semantics as path_starts_with:
// mutual prefixes imply identical component sequences.
pub fn path_equivalent(a: &Path, b: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        path_starts_with(a, b) && path_starts_with(b, a)
    }
    #[cfg(not(target_os = "windows"))]
    {
        a == b
    }
}

#[cfg(target_os = "windows")]
fn component_eq_ignore_case(a: &std::path::Component<'_>, b: &std::path::Component<'_>) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}

// Membership test for small CLI-supplied path sets (-X ignores, --collapse
// names): exact HashSet hit on Unix; case-folded linear scan on Windows.
// Sets are tiny, so the scan is negligible.
pub fn path_set_contains(set: &HashSet<PathBuf>, path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        set.iter().any(|p| path_equivalent(p, path))
    }
    #[cfg(not(target_os = "windows"))]
    {
        set.contains(path)
    }
}

// Case-folded component order for the simplify_dir_names sort; must agree
// with component_eq_ignore_case (equal folded components compare Equal) or
// prefix blocks would not stay contiguous.
#[cfg(target_os = "windows")]
fn cmp_components_ignore_case(
    mut i: std::path::Components<'_>,
    mut j: std::path::Components<'_>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    loop {
        match (i.next(), j.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = cmp_os_str_ignore_ascii_case(x.as_os_str(), y.as_os_str());
                if ord != Ordering::Equal {
                    return ord;
                }
            },
        }
    }
}

#[cfg(target_os = "windows")]
fn cmp_os_str_ignore_ascii_case(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> std::cmp::Ordering {
    let a = a.to_string_lossy();
    let b = b.to_string_lossy();
    let a = a.chars().map(|c| c.to_ascii_lowercase());
    let b = b.chars().map(|c| c.to_ascii_lowercase());
    a.cmp(b)
}

// Production code no longer calls this (see simplify_dir_names); kept as
// the documented parent-of semantics for the reference cross-check below
#[cfg(test)]
fn is_a_parent_of<P: AsRef<Path>>(parent: P, child: P) -> bool {
    let parent = parent.as_ref();
    let child = child.as_ref();
    path_starts_with(parent, child) && !path_starts_with(child, parent)
}

mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_simplify_dir() {
        let mut correct = HashSet::new();
        correct.insert(PathBuf::from("a"));
        assert_eq!(simplify_dir_names(&["a"]), correct);
    }

    #[test]
    fn test_simplify_dir_rm_subdir() {
        let mut correct = HashSet::new();
        correct.insert(["a", "b"].iter().collect::<PathBuf>());
        assert_eq!(simplify_dir_names(&["a/b/c", "a/b", "a/b/d/f"]), correct);
        assert_eq!(simplify_dir_names(&["a/b", "a/b/c", "a/b/d/f"]), correct);
    }

    #[test]
    fn test_simplify_dir_duplicates() {
        let mut correct = HashSet::new();
        correct.insert(["a", "b"].iter().collect::<PathBuf>());
        correct.insert(PathBuf::from("c"));
        assert_eq!(
            simplify_dir_names(&[
                "a/b",
                "a/b//",
                "a/././b///",
                "c",
                "c/",
                "c/.",
                "c/././",
                "c/././."
            ]),
            correct
        );
    }
    #[test]
    fn test_simplify_dir_rm_subdir_and_not_substrings() {
        let mut correct = HashSet::new();
        correct.insert(PathBuf::from("b"));
        correct.insert(["c", "a", "b"].iter().collect::<PathBuf>());
        correct.insert(["a", "b"].iter().collect::<PathBuf>());
        assert_eq!(simplify_dir_names(&["a/b", "c/a/b/", "b"]), correct);
    }

    #[test]
    fn test_simplify_dir_dots() {
        let mut correct = HashSet::new();
        correct.insert(PathBuf::from("src"));
        assert_eq!(simplify_dir_names(&["src/."]), correct);
    }

    #[test]
    fn test_simplify_dir_substring_names() {
        let mut correct = HashSet::new();
        correct.insert(PathBuf::from("src"));
        correct.insert(PathBuf::from("src_v2"));
        assert_eq!(simplify_dir_names(&["src/", "src_v2"]), correct);
    }

    #[test]
    fn test_simplify_dir_byte_order_trap() {
        // "a/b!x" must not be mistaken for a child of "a/b", and "a/b/c"
        // must still be dropped even with "a/b!x" sorting in between under
        // byte order
        let mut correct = HashSet::new();
        correct.insert(PathBuf::from("a/b"));
        correct.insert(PathBuf::from("a/b!x"));
        correct.insert(PathBuf::from("z"));
        assert_eq!(
            simplify_dir_names(&["a/b/c", "a/b!x", "a/b", "z", "a/b/d/e"]),
            correct
        );
    }

    // The original O(n^2) algorithm, kept to cross-check the sorted scan
    #[cfg(test)]
    fn simplify_dir_names_reference<P: AsRef<Path>>(dirs: &[P]) -> HashSet<PathBuf> {
        let mut top_level_names: HashSet<PathBuf> = HashSet::with_capacity(dirs.len());

        for t in dirs {
            let top_level_name = normalize_path(t);
            let mut can_add = true;
            let mut to_remove: Vec<PathBuf> = Vec::new();

            for tt in &top_level_names {
                if is_a_parent_of(&top_level_name, tt) {
                    to_remove.push(tt.clone());
                } else if is_a_parent_of(tt, &top_level_name)
                    || path_equivalent(tt, &top_level_name)
                {
                    // equivalent: same tree under a different case spelling
                    // (Windows); keep the first spelling seen
                    can_add = false;
                }
            }
            for r in to_remove {
                top_level_names.remove(&r);
            }
            if can_add {
                top_level_names.insert(top_level_name);
            }
        }

        top_level_names
    }

    #[test]
    fn test_simplify_dir_matches_reference_on_random_inputs() {
        // PERF-1 equivalence check; deterministic xorshift so failures
        // reproduce without a rand dependency
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let pool = [
            "a",
            "b",
            "c",
            "bb",
            "a/b",
            "a/b!x",
            "a/b/c",
            "a/bc",
            "x",
            "x/y",
            "x/y/z",
            "src",
            "src_v2",
            "src/v2",
            "a/./b",
            "a//b",
            "/a",
            "/a/b", // case variants (BUG-2)
            "C:/Temp/x",
            "c:/temp/x/sub",
            "B",
        ];
        let pool_len = u64::try_from(pool.len()).unwrap();
        for round in 0..500 {
            let count = usize::try_from(next() % 12).unwrap() + 1;
            let dirs: Vec<String> = (0..count)
                .map(|_| pool[usize::try_from(next() % pool_len).unwrap()].to_owned())
                .collect();
            assert_eq!(
                simplify_dir_names(&dirs),
                simplify_dir_names_reference(&dirs),
                "round {round}: {dirs:?}"
            );
        }
    }

    #[test]
    fn test_is_a_parent_of() {
        assert!(is_a_parent_of("/usr", "/usr/andy"));
        assert!(is_a_parent_of("/usr", "/usr/andy/i/am/descendant"));
        assert!(!is_a_parent_of("/usr", "/usr/."));
        assert!(!is_a_parent_of("/usr", "/usr/"));
        assert!(!is_a_parent_of("/usr", "/usr"));
        assert!(!is_a_parent_of("/usr/", "/usr"));
        assert!(!is_a_parent_of("/usr/andy", "/usr"));
        assert!(!is_a_parent_of("/usr/andy", "/usr/sibling"));
        assert!(!is_a_parent_of("/usr/folder", "/usr/folder_not_a_child"));
    }

    #[test]
    fn test_is_a_parent_of_root() {
        assert!(is_a_parent_of("/", "/usr/andy"));
        assert!(is_a_parent_of("/", "/usr"));
        assert!(!is_a_parent_of("/", "/"));
    }

    // BUG-2: Windows names match case-insensitively
    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_a_parent_of_case_insensitive() {
        assert!(is_a_parent_of("C:/Temp", "c:/TEMP/x"));
        assert!(is_a_parent_of("c:/temp/x", "C:/Temp/X/Y"));
        assert!(!is_a_parent_of("C:/Temp", "c:/temp"));
        assert!(!is_a_parent_of("C:/Temp", "c:/temp_other"));
    }

    // BUG-2: roots spelled with different cases are one tree, not two
    #[cfg(target_os = "windows")]
    #[test]
    fn test_simplify_dir_case_variants() {
        // equal up to case: first spelling wins
        let correct = HashSet::from([normalize_path("C:/Temp/x")]);
        assert_eq!(simplify_dir_names(&["C:/Temp/x", "c:/temp/x"]), correct);
        // case-variant parent subsumes the child spelling (parent wins, as
        // with the exact-case "a/b" + "a" case on Unix)
        let correct = HashSet::from([normalize_path("c:/temp/x")]);
        assert_eq!(simplify_dir_names(&["C:/Temp/x/sub", "c:/temp/x"]), correct);
    }
}
