use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    str,
};

use assert_cmd::{Command, cargo_bin_cmd};
use tempfile::{Builder, TempDir};

// File sizes differ on both platform and on the format of the disk.
// Windows: `ln` is not usually an available command; creation of symbolic links requires special
// enhanced permissions

fn build_temp_file(dir: &TempDir) -> PathBuf {
    let file_path = dir.path().join("notes.txt");
    let mut file = File::create(&file_path).unwrap();
    writeln!(file, "I am a temp file").unwrap();
    file_path
}

fn link_it(link_path: &Path, file_path_s: &str, is_soft: bool) -> String {
    let link_name_s = link_path.to_str().unwrap();
    let mut c = Command::new("ln");
    if is_soft {
        c.arg("-s");
    }
    c.arg(file_path_s);
    c.arg(link_name_s);
    assert!(c.output().is_ok());
    link_name_s.into()
}

#[cfg_attr(
    target_os = "windows",
    ignore = "creating symlinks on windows requires elevated privileges"
)]
#[test]
pub fn test_soft_sym_link() {
    let dir = Builder::new().tempdir().unwrap();
    let file = build_temp_file(&dir);
    let dir_s = dir.path().to_str().unwrap();
    let file_path_s = file.to_str().unwrap();

    let link_name = dir.path().join("the_link");
    let link_name_s = link_it(&link_name, file_path_s, true);

    let c = format!(" ├── {link_name_s}");
    let b = format!(" ┌── {file_path_s}");
    let a = format!("─┴ {dir_s}");

    let mut cmd = cargo_bin_cmd!("zdu");
    // Mac test runners create long filenames in tmp directories
    let output = cmd
        .args(["-p", "-c", "-s", "-w", "999", dir_s])
        .unwrap()
        .stdout;

    let output = str::from_utf8(&output).unwrap();

    assert!(output.contains(a.as_str()));
    assert!(output.contains(b.as_str()));
    assert!(output.contains(c.as_str()));
}

#[cfg_attr(
    target_os = "windows",
    ignore = "creating symlinks on windows requires elevated privileges"
)]
#[test]
pub fn test_hard_sym_link() {
    let dir = Builder::new().tempdir().unwrap();
    let file = build_temp_file(&dir);
    let dir_s = dir.path().to_str().unwrap();
    let file_path_s = file.to_str().unwrap();

    let link_name = dir.path().join("the_link");
    link_it(&link_name, file_path_s, false);

    let dirs_output = format!("─┴ {dir_s}");
    let link_name_s = link_name.to_str().unwrap();

    let mut cmd = cargo_bin_cmd!("zdu");
    // Mac test runners create long filenames in tmp directories
    let output = cmd.args(["-p", "-c", "-w", "999", dir_s]).unwrap().stdout;

    // Hardlink dedup keeps exactly one of the two names. Which one survives
    // is first-seen and depends on readdir order (same semantics as du);
    // the previous deterministic winner came from the removed post-walk
    // inode sort.
    let output = str::from_utf8(&output).unwrap();
    assert!(output.contains(dirs_output.as_str()));
    let file_shown = output.contains(file_path_s);
    let link_shown = output.contains(link_name_s);
    assert!(
        file_shown ^ link_shown,
        "exactly one hardlink name must survive, got file={file_shown} link={link_shown}"
    );
}

#[cfg_attr(
    target_os = "windows",
    ignore = "creating symlinks on windows requires elevated privileges"
)]
#[test]
pub fn test_hard_sym_link_no_dup_multi_arg() {
    let dir = Builder::new().tempdir().unwrap();
    let dir_link = Builder::new().tempdir().unwrap();
    let file = build_temp_file(&dir);
    let dir_s = dir.path().to_str().unwrap();
    let dir_link_s = dir_link.path().to_str().unwrap();
    let file_path_s = file.to_str().unwrap();

    let link_name = dir_link.path().join("the_link");
    let link_name_s = link_it(&link_name, file_path_s, false);

    let mut cmd = cargo_bin_cmd!("zdu");

    // Mac test runners create long filenames in tmp directories
    let output = cmd
        .args(["-p", "-c", "-w", "999", "-b", dir_link_s, dir_s])
        .unwrap()
        .stdout;

    // The link or the file should appear but not both
    let output = str::from_utf8(&output).unwrap();
    let has_file_only = output.contains(file_path_s) && !output.contains(&link_name_s);
    let has_link_only = !output.contains(file_path_s) && output.contains(&link_name_s);
    assert!(has_file_only || has_link_only);
}

#[cfg_attr(
    target_os = "windows",
    ignore = "creating symlinks on windows requires elevated privileges"
)]
#[test]
pub fn test_recursive_sym_link() {
    let dir = Builder::new().tempdir().unwrap();
    let dir_s = dir.path().to_str().unwrap();

    let link_name = dir.path().join("the_link");
    let link_name_s = link_it(&link_name, dir_s, true);

    let a = format!("─┬ {dir_s}");
    let b = format!(" └── {link_name_s}");

    let mut cmd = cargo_bin_cmd!("zdu");
    let output = cmd
        .arg("-p")
        .arg("-c")
        .arg("-r")
        .arg("-s")
        .arg("-w")
        .arg("999")
        .arg(dir_s)
        .unwrap()
        .stdout;
    let output = str::from_utf8(&output).unwrap();

    assert!(output.contains(a.as_str()));
    assert!(output.contains(b.as_str()));
}

// Following a directory link that loops back to an ancestor must not expand
// the loop: each filesystem object is descended into at most once.
#[cfg(not(target_os = "windows"))]
#[test]
pub fn test_sym_link_dir_loop_with_dereference() {
    let dir = Builder::new().tempdir().unwrap();
    let dir_s = dir.path().to_str().unwrap();

    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("big.bin"), vec![0u8; 100_000]).unwrap();

    let link_name = dir.path().join("loop");
    link_it(&link_name, dir_s, true);

    let mut cmd = cargo_bin_cmd!("zdu");
    let output = cmd
        .args(["-p", "-c", "-s", "-w", "999", "-L", dir_s])
        .unwrap();
    assert!(output.status.success());
    let stdout = str::from_utf8(&output.stdout).unwrap();
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(!stdout.contains("loop/loop"), "loop expanded: {stdout}");
    assert!(
        !stderr.contains("No such file or directory"),
        "loop ran past path limits: {stderr}"
    );
}

// BUG-11: a symlink pointing back to the walk root itself must be treated
// as a loop under -L. Without seeding the root's (dev,ino) into the
// followed-dir set, the whole subtree was walked and counted a second time
// under -p (where the global hardlink dedup can't mask it).
#[cfg(not(target_os = "windows"))]
#[test]
pub fn test_sym_link_to_root_not_walked_twice() {
    let dir = Builder::new().tempdir().unwrap();
    let dir_s = dir.path().to_str().unwrap();

    std::fs::write(dir.path().join("big.bin"), vec![0u8; 100_000]).unwrap();
    let self_link = dir.path().join("self");
    link_it(&self_link, dir_s, true);

    let mut cmd = cargo_bin_cmd!("zdu");
    let output = cmd
        .args(["-s", "-c", "-p", "-w", "999", "-L", dir_s])
        .unwrap();
    assert!(output.status.success());
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert_eq!(
        stdout.matches("big.bin").count(),
        1,
        "root self-loop walked twice: {stdout}"
    );
}

// Windows junctions need no elevated privileges, unlike symlinks. `mklink /J`
// creates the same kind of reparse-point loop that `C:\Users` trees contain.
#[cfg(target_os = "windows")]
#[test]
pub fn test_junction_loop_with_dereference() {
    use std::process::Command as OsCommand;

    let dir = Builder::new().tempdir().unwrap();
    let dir_s = dir.path().to_str().unwrap();

    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("big.bin"), vec![0u8; 100_000]).unwrap();

    let loop_junction = dir.path().join("loop");
    let status = OsCommand::new("cmd")
        .args(["/c", "mklink", "/J", loop_junction.to_str().unwrap(), dir_s])
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");

    let mut cmd = cargo_bin_cmd!("zdu");
    let output = cmd
        .args(["-p", "-c", "-s", "-w", "999", "-L", dir_s])
        .unwrap();
    assert!(output.status.success());
    let stdout = str::from_utf8(&output.stdout).unwrap();
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(!stdout.contains("loop\\loop"), "loop expanded: {stdout}");
    assert!(
        !stderr.contains("No such file or directory"),
        "loop ran past path limits: {stderr}"
    );
}

// BUG-1 regression: under -L a junction to a plain directory used to vanish
// entirely. The cheap Windows metadata path returns no file id, and the
// walker treated the missing id as "drop the entry".
#[cfg(target_os = "windows")]
#[test]
pub fn test_junction_dereference_shows_target_content() {
    use std::process::Command as OsCommand;

    let dir = Builder::new().tempdir().unwrap();

    let target = dir.path().join("jt");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("f.txt"), b"123456789").unwrap();

    let junction = dir.path().join("jlink");
    let status = OsCommand::new("cmd")
        .args([
            "/c",
            "mklink",
            "/J",
            junction.to_str().unwrap(),
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");

    let mut cmd = cargo_bin_cmd!("zdu");
    let output = cmd
        .args([
            "-L",
            "-p",
            "-c",
            "-s",
            "-w",
            "999",
            "-d",
            "2",
            dir.path().to_str().unwrap(),
        ])
        .unwrap();
    assert!(output.status.success());
    let stdout = str::from_utf8(&output.stdout).unwrap();

    // Both the real dir and the followed junction must expose f.txt.
    // stfu8 escapes path separators in the tree output, hence the doubled \.
    assert!(
        stdout.contains("jt\\\\f.txt"),
        "target content missing: {stdout}"
    );
    assert!(
        stdout.contains("jlink\\\\f.txt"),
        "followed junction content missing: {stdout}"
    );
}

// BUG-13 + BUG-17 interaction: a -L followed link whose target is missing
// is reported exactly once, as file_not_found. The initial stat of the link
// fails too, which used to pile a second, contradictory
// "Could not get metadata" message on top.
#[cfg(target_os = "windows")]
#[test]
pub fn test_dangling_junction_with_dereference_reported_once() {
    use std::process::Command as OsCommand;

    let dir = Builder::new().tempdir().unwrap();

    let junction = dir.path().join("nowhere");
    let status = OsCommand::new("cmd")
        .args([
            "/c",
            "mklink",
            "/J",
            junction.to_str().unwrap(),
            // Junctions can be created without the target existing
            r"C:\nonexistent-zdu-test-target",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");

    let mut cmd = cargo_bin_cmd!("zdu");
    cmd.args(["-L", "-p", "-c", "-w", "999", dir.path().to_str().unwrap()]);
    // Nonzero: the empty tree next to a file_not_found root failure is
    // BUG-17's exit-1 condition
    let output_error = cmd.unwrap_err();
    let result = output_error.as_output().unwrap();
    let stderr = str::from_utf8(&result.stderr).unwrap();
    assert!(
        stderr.contains("No such file or directory"),
        "expected the classification message, got: {stderr}"
    );
    assert!(
        !stderr.contains("Could not get metadata"),
        "dangling followed link must not double-report, got: {stderr}"
    );
}

// BUG-3 regression: on Windows the default (allocated) mode used the logical
// size and -s the on-disk size, i.e. the two modes were swapped. A sparse
// file exposes the difference: logical 1 MiB, allocated ~0.
#[cfg(target_os = "windows")]
#[test]
pub fn test_sparse_file_allocated_vs_apparent_size() {
    use std::process::Command as OsCommand;

    let dir = Builder::new().tempdir().unwrap();
    let sp = dir.path().join("sp.bin");
    std::fs::write(&sp, vec![0u8; 25]).unwrap();

    // fsutil needs no elevation for a sparse flag on a file we own; skip the
    // test on filesystems that do not support it (e.g. FAT/exFAT temp dirs)
    let flagged = OsCommand::new("fsutil")
        .args(["sparse", "setflag", sp.to_str().unwrap()])
        .status()
        .is_ok_and(|s| s.success());
    if !flagged {
        eprintln!("skipping: fsutil sparse setflag unavailable");
        return;
    }
    // extend the logical size without allocating anything
    std::fs::OpenOptions::new()
        .write(true)
        .open(&sp)
        .unwrap()
        .set_len(1 << 20)
        .unwrap();

    let run = |args: &[&str]| {
        let mut cmd = cargo_bin_cmd!("zdu");
        let output = cmd
            .args([
                "-p",
                "-c",
                "-b",
                "-d",
                "1",
                "-o",
                "b",
                "-w",
                "999",
                dir.path().to_str().unwrap(),
            ])
            .args(args)
            .unwrap();
        let stdout = str::from_utf8(&output.stdout).unwrap().to_owned();
        let sp_line = stdout
            .lines()
            .find(|l| l.contains("sp.bin"))
            .unwrap_or_else(|| panic!("sp.bin missing from output: {stdout}"));
        sp_line
            .trim()
            .split('B')
            .next()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or_else(|| panic!("cannot parse size from line: {sp_line}"))
    };

    // -s: apparent size = logical 1 MiB
    assert_eq!(run(&["-s"]), 1 << 20);
    // default: allocated size must be well below logical (all-zero sparse
    // file; 0 on NTFS, up to 64 Ki on volumes with coarser sparse support)
    assert!(run(&[]) < 512 * 1024);
}

// BUG-14 regression: like GNU du -x, command-line arguments are stat'ed with
// follow semantics, so -x with a symlink argument must filter by the
// *target's* volume. Pre-fix, the link's own volume was collected and the
// target's contents were filtered to nothing when the two differed.
// Needs the symlink and its target on different filesystems: /dev/shm
// (tmpfs) vs the tempdir's real filesystem; skips when they coincide.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
pub fn test_limit_filesystem_with_symlink_arg() {
    use std::os::unix::fs::MetadataExt;

    let target = Builder::new().tempdir().unwrap();
    std::fs::write(target.path().join("f.bin"), vec![0u8; 4096]).unwrap();

    let shm = Path::new("/dev/shm");
    let (shm_dev, target_dev) =
        if let (Ok(a), Ok(b)) = (std::fs::metadata(shm), target.path().metadata()) {
            (a.dev(), b.dev())
        } else {
            eprintln!("skipping: /dev/shm or tempdir not statable");
            return;
        };
    if shm_dev == target_dev {
        eprintln!("skipping: /dev/shm and tempdir share one filesystem");
        return;
    }

    let link = shm.join(format!("zdu_link_{}", std::process::id()));
    std::os::unix::fs::symlink(target.path(), &link).unwrap();

    let mut cmd = cargo_bin_cmd!("zdu");
    let output = cmd
        .args([
            "-x",
            "-s",
            "-c",
            "-w",
            "999",
            "-d",
            "1",
            link.to_str().unwrap(),
        ])
        .unwrap();
    let _ = std::fs::remove_file(&link);
    assert!(output.status.success());
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("f.bin"),
        "-x filtered the target contents through a symlink argument: {stdout}"
    );
}
