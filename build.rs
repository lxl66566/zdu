use std::{env, fs, fs::File, io::Error, path::PathBuf};

use clap::CommandFactory;
use clap_complete::{generate_to, shells::*};
use clap_mangen::Man;

include!("src/cli.rs");

fn main() -> Result<(), Error> {
    // BUILD-1: generated assets always go to OUT_DIR, never the source tree.
    // A plain `cargo build` on a read-only checkout (rpmbuild/deb/Nix) used
    // to fail when writing ./share; the release workflow now copies
    // OUT_DIR/share out for packaging instead.
    // Declaring rerun-if-changed opts the script into re-running only when
    // these files change; otherwise cargo re-runs it (5 completions + man
    // render) after every incremental edit anywhere in the package.
    // src/cli.rs is included! below, so it must be listed too.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli.rs");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set for build scripts"));
    // Generated assets use the same relative layout as their Linux install
    // locations (relative to a prefix like /usr or /usr/local), so the release
    // archive can be extracted directly into a prefix.
    // PowerShell has no standard install dir; it follows the share/ layout for
    // consistency anyway.
    let man_dir = out_dir.join("share/man/man1");
    let bash_dir = out_dir.join("share/bash-completion/completions");
    let zsh_dir = out_dir.join("share/zsh/site-functions");
    let fish_dir = out_dir.join("share/fish/vendor_completions.d");
    let elvish_dir = out_dir.join("share/elvish/lib");
    let ps_dir = out_dir.join("share/powershell/completions");
    for dir in [
        &man_dir,
        &bash_dir,
        &zsh_dir,
        &fish_dir,
        &elvish_dir,
        &ps_dir,
    ] {
        fs::create_dir_all(dir)?;
    }

    let app_name = "zdu";
    let mut cmd = Cli::command();

    generate_to(Bash, &mut cmd, app_name, &bash_dir)?;
    // bash-completion expects the file named after the command, without an extension
    fs::rename(bash_dir.join("zdu.bash"), bash_dir.join(app_name))?;
    generate_to(Zsh, &mut cmd, app_name, &zsh_dir)?;
    generate_to(Fish, &mut cmd, app_name, &fish_dir)?;
    generate_to(Elvish, &mut cmd, app_name, &elvish_dir)?;
    generate_to(PowerShell, &mut cmd, app_name, &ps_dir)?;

    let mut file = File::create(man_dir.join("zdu.1"))?;
    Man::new(cmd).render(&mut file)?;

    Ok(())
}
