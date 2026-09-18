use std::{
    collections::{BinaryHeap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use stfu8::encode_u8;

use crate::{
    display::get_printable_name,
    display_node::DisplayNode,
    node::{FileTime, Node},
    utils::path_set_contains,
};

// Aggregation options are boolean display switches by nature
#[allow(clippy::struct_excessive_bools)]
pub struct AggregateData {
    pub min_size: Option<usize>,
    pub only_dir: bool,
    pub only_file: bool,
    pub number_of_lines: usize,
    pub depth: usize,
    pub using_a_filter: bool,
    pub short_paths: bool,
}

pub fn get_biggest(
    top_level_nodes: Vec<Node>,
    display_data: &AggregateData,
    by_filetime: Option<&FileTime>,
    keep_collapsed: &HashSet<PathBuf>,
) -> DisplayNode {
    let mut heap = BinaryHeap::new();
    let number_top_level_nodes = top_level_nodes.len();
    let root;

    if number_top_level_nodes == 0 {
        root = total_node_builder(0, vec![]);
    } else if number_top_level_nodes > 1 {
        let size = if by_filetime.is_some() {
            top_level_nodes
                .iter()
                .map(|node| node.size)
                .max()
                .unwrap_or(0)
        } else {
            top_level_nodes.iter().map(|node| node.size).sum()
        };

        let nodes = handle_duplicate_top_level_names(top_level_nodes, display_data.short_paths);
        root = total_node_builder(size, nodes);
        heap = always_add_children(display_data, &root, heap);
    } else {
        root = top_level_nodes.into_iter().next().unwrap();
        heap = add_children(display_data, &root, heap);
    }

    fill_remaining_lines(heap, &root, display_data, keep_collapsed)
}

fn total_node_builder(size: u64, children: Vec<Node>) -> Node {
    Node {
        name: PathBuf::from("(total)"),
        size,
        children,
        inode_device: None,
        depth: 0,
        is_file: false,
    }
}

pub fn fill_remaining_lines<'a>(
    mut heap: BinaryHeap<&'a Node>,
    root: &'a Node,
    display_data: &AggregateData,
    keep_collapsed: &HashSet<PathBuf>,
) -> DisplayNode {
    let mut allowed_nodes = HashMap::new();

    while allowed_nodes.len() < display_data.number_of_lines {
        let line = heap.pop();
        match line {
            Some(line) => {
                // If we are not doing only_file OR if we are doing
                // only_file and it has no children (ie is a file not a dir)
                if !display_data.only_file || line.children.is_empty() {
                    allowed_nodes.insert(line.name.as_path(), line);
                }
                // BUG-2: case-folded on Windows so `-c DIR` collapses `dir`
                if !path_set_contains(keep_collapsed, &line.name) {
                    heap = add_children(display_data, line, heap);
                }
            },
            None => break,
        }
    }

    if display_data.only_file {
        flat_rebuilder(allowed_nodes, root)
    } else {
        recursive_rebuilder(&allowed_nodes, root)
    }
}

fn add_children<'a>(
    display_data: &AggregateData,
    file_or_folder: &'a Node,
    heap: BinaryHeap<&'a Node>,
) -> BinaryHeap<&'a Node> {
    if display_data.depth > file_or_folder.depth {
        always_add_children(display_data, file_or_folder, heap)
    } else {
        heap
    }
}

fn always_add_children<'a>(
    display_data: &AggregateData,
    file_or_folder: &'a Node,
    mut heap: BinaryHeap<&'a Node>,
) -> BinaryHeap<&'a Node> {
    heap.extend(
        file_or_folder
            .children
            .iter()
            .filter(|c| match display_data.min_size {
                Some(ms) => c.size > ms as u64,
                None => !display_data.using_a_filter || c.is_file || c.size > 0,
            })
            // PERF-4: classify from the walker-known is_file instead of a
            // follow-stat per node (and its TOCTOU). Semantic change: -D now
            // keeps non-regular entries, so symlinks (including dangling
            // ones and links to files) appear where the old is_dir() dropped
            // them. Under -L a symlink to a dir was already kept (and walked
            // as one), so nothing changes there. This matches zdu's own -L
            // interpretation and avoids re-stat-ing every node; upstream
            // dust still uses follow-semantic is_dir() here.
            .filter(|c| !display_data.only_dir || !c.is_file),
    );
    heap
}

// Finds children of current, if in allowed_nodes adds them as children to new DisplayNode
fn recursive_rebuilder(allowed_nodes: &HashMap<&Path, &Node>, current: &Node) -> DisplayNode {
    let new_children: Vec<_> = current
        .children
        .iter()
        .filter(|c| allowed_nodes.contains_key(c.name.as_path()))
        .map(|c| recursive_rebuilder(allowed_nodes, c))
        .collect();

    build_display_node(new_children, current)
}

// Applies all allowed nodes as children to current node
fn flat_rebuilder(allowed_nodes: HashMap<&Path, &Node>, current: &Node) -> DisplayNode {
    let new_children: Vec<DisplayNode> = allowed_nodes
        .into_values()
        .map(|v| DisplayNode {
            name: v.name.clone(),
            size: v.size,
            children: vec![],
            is_file: v.is_file,
        })
        .collect::<Vec<DisplayNode>>();
    build_display_node(new_children, current)
}

fn build_display_node(mut new_children: Vec<DisplayNode>, current: &Node) -> DisplayNode {
    new_children.sort_by(|lhs, rhs| lhs.cmp(rhs).reverse());
    DisplayNode {
        name: current.name.clone(),
        size: current.size,
        children: new_children,
        is_file: current.is_file,
    }
}

fn names_have_dup(top_level_nodes: &Vec<Node>) -> bool {
    let mut stored = HashSet::new();
    for node in top_level_nodes {
        let name = get_printable_name(&node.name, true);
        if stored.contains(&name) {
            return true;
        }
        stored.insert(name);
    }
    false
}

fn handle_duplicate_top_level_names(
    mut top_level_nodes: Vec<Node>,
    short_paths: bool,
) -> Vec<Node> {
    // If we have top level names that are the same - we need to tweak them:
    // PERF-8: only the displayed name changes per round; the subtrees used
    // to be deep-cloned (one whole-tree clone up front, then per-node child
    // clones for up to 10 rounds). Mutating just `name` in place is
    // equivalent: each round appends "({ancestor})" to the last component,
    // which never introduces new components, so later rounds walk the same
    // component sequence the cloned version did.
    if short_paths && names_have_dup(&top_level_nodes) {
        let mut dir_walk_up_count = 0;

        while names_have_dup(&top_level_nodes) && dir_walk_up_count < 10 {
            dir_walk_up_count += 1;
            for node in &mut top_level_nodes {
                let mut folders = node.name.iter().rev();
                // Get parent folder (if second time round get grandparent and so on)
                for _ in 0..dir_walk_up_count {
                    folders.next();
                }
                // Add (parent_name) to path of Node; nodes without enough
                // ancestors keep their name (previously cloned unchanged)
                if let Some(data) = folders.next() {
                    let parent = encode_u8(data.as_encoded_bytes());
                    let current_node = node.name.display();
                    node.name = PathBuf::from(format!("{current_node}({parent})"));
                }
            }
        }
    }
    top_level_nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, size: u64, is_file: bool) -> Node {
        Node {
            name: PathBuf::from(name),
            size,
            children: vec![],
            inode_device: None,
            depth: 1,
            is_file,
        }
    }

    fn aggregate(only_dir: bool) -> AggregateData {
        AggregateData {
            min_size: None,
            only_dir,
            only_file: false,
            number_of_lines: 10,
            depth: usize::MAX,
            using_a_filter: false,
            short_paths: true,
        }
    }

    // PERF-4 semantics: -D classifies by the walker-known is_file, no stat.
    // The link path need not exist on disk at all - the old is_dir() would
    // have dropped it (stat misses / target is a file).
    #[test]
    fn only_dir_keeps_dirs_and_links_drops_files() {
        let nodes = vec![
            node("real_dir", 30, false),
            node("a_link", 20, false), // symlink or dangling: is_file = false
            node("plain_file", 10, true),
        ];
        let root = get_biggest(nodes, &aggregate(true), None, &HashSet::new());
        let names: Vec<String> = root
            .children
            .iter()
            .map(|c| c.name.display().to_string())
            .collect();
        assert_eq!(names, vec!["real_dir".to_owned(), "a_link".to_owned()]);
    }

    #[test]
    fn without_only_dir_all_entries_kept() {
        let nodes = vec![node("real_dir", 30, false), node("plain_file", 10, true)];
        let root = get_biggest(nodes, &aggregate(false), None, &HashSet::new());
        assert_eq!(root.children.len(), 2);
    }

    // PERF-8 rewrite: renaming must walk one ancestor further per round and
    // leave everything but `name` (sizes, children identity) untouched
    #[test]
    fn duplicate_top_level_names_disambiguated_by_ancestors() {
        let mut a = node("p/x/dup", 40, false);
        a.children = vec![node("p/x/dup/inner", 5, true)];
        let b = node("q/x/dup", 30, false);

        let out = handle_duplicate_top_level_names(vec![a, b], true);
        let names: Vec<String> = out.iter().map(|n| n.name.display().to_string()).collect();
        // round 1 appends the parent ("x" for both, dup remains), round 2
        // reaches the differing grandparent ("p" vs "q")
        assert_eq!(names, vec![
            "p/x/dup(x)(p)".to_owned(),
            "q/x/dup(x)(q)".to_owned()
        ]);
        // subtrees ride along without being copied/rebuilt
        assert_eq!(out[0].children[0].name, PathBuf::from("p/x/dup/inner"));
        assert_eq!(out[0].size, 40);
    }

    #[test]
    fn no_dup_or_full_paths_leaves_names_untouched() {
        let out = handle_duplicate_top_level_names(
            vec![node("p/a", 1, false), node("q/b", 1, false)],
            true,
        );
        assert_eq!(out[0].name, PathBuf::from("p/a"));

        let out = handle_duplicate_top_level_names(
            vec![node("p/a", 1, false), node("q/a", 1, false)],
            false,
        );
        assert_eq!(out[1].name, PathBuf::from("q/a"));
    }
}
