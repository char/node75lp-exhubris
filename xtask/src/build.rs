use std::{fs, io::Read, path::Path};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::{Toolchain, upstream};

const APP: &str = "node75";
const CONFIG: &str = "node75.kdl";

pub fn build(root: &Path, toolchain: &Toolchain) -> Result<()> {
    upstream::ensure_upstreams(root, false)?;

    let exhubris = root.join("vendor").join("exhubris");
    crate::run_command(
        crate::command(toolchain, &toolchain.cargo)
            .current_dir(root)
            .arg("build")
            .arg("--manifest-path")
            .arg(exhubris.join("Cargo.toml"))
            .args([
                "--locked",
                "-p",
                "hubris-build",
                "--target",
                &toolchain.host,
            ]),
        "hubris-build build",
    )?;

    let out = root.join("target").join(APP);
    fs::create_dir_all(&out)?;
    let bundle = out.join(format!("{APP}-build.zip"));
    let builder = exhubris
        .join("target")
        .join(&toolchain.host)
        .join("debug")
        .join(crate::exe("hubris-build"));
    crate::run_command(
        crate::command(toolchain, &builder)
            .current_dir(root)
            .env("HUBRIS_PROJECT_ROOT", root)
            .arg("build")
            .arg(CONFIG)
            .arg("--out")
            .arg(&bundle),
        "exhubris application build",
    )?;

    extract_bundle(&bundle, &out)?;
    let image = out.join(format!("{APP}.experimental.bin"));
    crate::run_command(
        crate::command(toolchain, &toolchain.objcopy)
            .args(["-O", "binary"])
            .arg(out.join("combined.elf"))
            .arg(&image),
        "combined image extraction",
    )?;

    let bytes = fs::read(&image)?;
    println!(
        "Built {} ({} bytes, SHA-256 {})",
        image.display(),
        bytes.len(),
        Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    Ok(())
}

/// Unpack the ELFs the bundle carries, named after the role they play.
fn extract_bundle(bundle: &Path, out: &Path) -> Result<()> {
    let file = fs::File::open(bundle)
        .with_context(|| format!("opening build bundle {}", bundle.display()))?;
    let mut archive = zip::ZipArchive::new(file)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let dest = match entry.name() {
            "img/final.elf" => "combined.elf".to_owned(),
            "elf/kernel" => "kernel.elf".to_owned(),
            name => match name.strip_prefix("elf/task/") {
                Some(task) => format!("{task}.elf"),
                None => continue,
            },
        };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        fs::write(out.join(dest), bytes)?;
    }
    Ok(())
}
