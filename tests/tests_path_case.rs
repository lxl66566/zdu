// BUG-2 integration: Windows filesystems match path names case-insensitively,
// so roots and -X/--collapse names spelled with a different case must still
// refer to the same directory. Unix is case-sensitive by design; these tests
// would be wrong there.
#![cfg(target_os = "windows")]

use std::fs;

use assert_cmd::cargo_bin_cmd;
use tempfile::Builder;

// root/
//   keep.txt
//   dir_a/inner.txt
fn setup_tree() -> tempfile::TempDir {
    let dir = Builder::new().tempdir().unwrap();
    fs::write(dir.path().join("keep.txt"), b"keep me").unwrap();
    fs::create_dir(dir.path().join("dir_a")).unwrap();
    fs::write(dir.path().join("dir_a").join("inner.txt"), b"inner").unwrap();
    dir
}

fn run_stdout(args: &[&str]) -> String {
    let mut cmd = cargo_bin_cmd!("zdu");
    let output = cmd.args(args).unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

// The same directory passed twice with different case spellings is one tree:
// each file must appear exactly once (pre-fix it was fully double-counted,
// since Windows fast-path metadata has no id to fall back on for root dedup).
#[test]
fn test_case_variant_roots_counted_once() {
    let dir = setup_tree();
    let lower = dir.path().to_string_lossy().into_owned();
    let upper = lower.to_uppercase();
    assert_ne!(lower, upper);

    let stdout = run_stdout(&["-s", "-c", "-w", "999", lower.as_str(), upper.as_str()]);
    assert_eq!(
        stdout.matches("inner.txt").count(),
        1,
        "case-variant root double-counted: {stdout}"
    );
}

// -X DIR_A must ignore dir_a just like -X dir_a does.
#[test]
fn test_ignore_directory_matches_case_insensitively() {
    let dir = setup_tree();
    let root = dir.path().to_string_lossy().into_owned();

    // sanity: without -X the entry is shown
    let stdout = run_stdout(&["-s", "-c", "-w", "999", root.as_str()]);
    assert!(stdout.contains("inner.txt"));

    for ignore in ["dir_a", "DIR_A", "Dir_A"] {
        let stdout = run_stdout(&["-s", "-c", "-w", "999", "-X", ignore, root.as_str()]);
        assert!(
            !stdout.contains("inner.txt"),
            "-X {ignore} failed to ignore dir_a: {stdout}"
        );
        assert!(
            stdout.contains("keep.txt"),
            "-X {ignore} must not drop other entries: {stdout}"
        );
    }
}
