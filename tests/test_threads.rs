// Concurrency consistency: the parallel walk must produce byte-identical
// -j output regardless of the thread count. Aggregation (size folding in
// finalize_chain) and sorting must not depend on completion order.
// Uses -s (apparent) so the hardlink first-seen-wins dedup cannot introduce
// its documented nondeterminism (that is pre-existing, thread-count-agnostic
// behavior, not what this test pins down).
use std::fs;

use assert_cmd::cargo_bin_cmd;
use tempfile::Builder;

// root/
//   d0..d3/            file{i}.txt with varying sizes
//   d0/deep/a/a/.../a  20-level nesting
//   d1/dup.txt         same file name in several dirs (path-keyed aggregation)
fn build_tree() -> tempfile::TempDir {
    let dir = Builder::new().tempdir().unwrap();
    for i in 0..4 {
        let d = dir.path().join(format!("d{i}"));
        fs::create_dir(&d).unwrap();
        for j in 0..30 {
            fs::write(d.join(format!("file{j}.txt")), vec![
                0u8;
                100 * (i * 30 + j + 1)
            ])
            .unwrap();
        }
        fs::write(d.join("dup.txt"), vec![0u8; 123]).unwrap();
    }
    let mut deep = dir.path().join("d0/deep");
    fs::create_dir(&deep).unwrap();
    for _ in 0..20 {
        deep.push("a");
        fs::create_dir(&deep).unwrap();
    }
    fs::write(deep.join("bottom.txt"), b"bottom").unwrap();
    dir
}

fn run_json(args: &[&str], root: &str) -> String {
    let mut cmd = cargo_bin_cmd!("zdu");
    let output = cmd
        .args(["-s", "-j", "-P", "-w", "999"])
        .args(args)
        .arg(root)
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn json_output_is_identical_across_thread_counts() {
    let dir = build_tree();
    let root = dir.path().to_string_lossy().into_owned();

    let single = run_json(&["-T", "1"], &root);
    let quad = run_json(&["-T", "4"], &root);
    let many = run_json(&["-T", "16"], &root);
    let default = run_json(&[], &root);

    assert_eq!(single, quad, "-T 1 vs -T 4 differ");
    assert_eq!(single, many, "-T 1 vs -T 16 differ");
    assert_eq!(single, default, "-T 1 vs default differ");
    assert!(single.contains("bottom.txt"), "tree incomplete: {single}");
}

// The rendered tree must be stable too: same rows, same order, same bars.
#[test]
fn tree_output_is_identical_across_thread_counts() {
    let dir = build_tree();
    let root = dir.path().to_string_lossy().into_owned();

    let run = |threads: &str| {
        let mut cmd = cargo_bin_cmd!("zdu");
        let output = cmd
            .args([
                "-s", "-c", "-P", "-w", "120", "-d", "3", "-T", threads, &root,
            ])
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };

    // 5 repeated -T 16 runs: a flaky interleaving fails at least once
    let single = run("1");
    for _ in 0..5 {
        assert_eq!(single, run("16"), "-T 1 vs repeated -T 16 differ");
    }
}
