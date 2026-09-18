use std::{
    env, fs,
    fs::File,
    io::Error,
    path::{Path, PathBuf},
};

use clap::CommandFactory;
use clap_complete::{generate_to, shells::*};
use clap_mangen::Man;

include!("src/cli.rs");

// `cargo package`/`publish` verifies the tarball by building under
// target/package; a build script must not modify the source tree there,
// so generated assets go to OUT_DIR instead of the repo directories.
fn packaging_verify() -> bool {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default();
    let comps: Vec<_> = Path::new(&manifest_dir).components().collect();
    comps
        .windows(2)
        .any(|w| w[0].as_os_str() == "target" && w[1].as_os_str() == "package")
}

fn main() -> Result<(), Error> {
    // Generated assets use the same relative layout as their Linux install
    // locations (relative to a prefix like /usr or /usr/local), so the release
    // archive can be extracted directly into a prefix.
    // PowerShell has no standard install dir; it follows the share/ layout for
    // consistency anyway.
    let root: PathBuf = if packaging_verify() {
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set for build scripts"))
    } else {
        ".".into()
    };
    let man_dir = root.join("share/man/man1");
    let bash_dir = root.join("share/bash-completion/completions");
    let zsh_dir = root.join("share/zsh/site-functions");
    let fish_dir = root.join("share/fish/vendor_completions.d");
    let elvish_dir = root.join("share/elvish/lib");
    let ps_dir = root.join("share/powershell/completions");
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
