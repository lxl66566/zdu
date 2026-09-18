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
