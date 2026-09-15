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
    let (outdir, man_dir): (PathBuf, PathBuf) = if packaging_verify() {
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set for build scripts"));
        (out.join("completions"), out.join("man-page"))
    } else {
        ("completions".into(), "man-page".into())
    };
    fs::create_dir_all(&outdir)?;
    let app_name = "zdu";
    let mut cmd = Cli::command();

    generate_to(Bash, &mut cmd, app_name, &outdir)?;
    generate_to(Zsh, &mut cmd, app_name, &outdir)?;
    generate_to(Fish, &mut cmd, app_name, &outdir)?;
    generate_to(PowerShell, &mut cmd, app_name, &outdir)?;
    generate_to(Elvish, &mut cmd, app_name, &outdir)?;

    let file = man_dir.join("zdu.1");
    fs::create_dir_all(&man_dir)?;
    let mut file = File::create(file)?;

    Man::new(cmd).render(&mut file)?;

    Ok(())
}
