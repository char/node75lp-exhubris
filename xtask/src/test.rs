use std::path::Path;

use anyhow::Result;

use crate::{Toolchain, command, run_command, upstream};

pub fn test(root: &Path, toolchain: &Toolchain) -> Result<()> {
    upstream::ensure_upstreams(root, false)?;

    // Host tests for this repo's lib crates (includes xtask itself).
    run_command(
        command(toolchain, &toolchain.cargo)
            .current_dir(root)
            .args(["test", "--locked"]),
        "host tests",
    )?;

    // The upstream build tools' own test suites, at the pinned revisions.
    let exhubris = root.join("vendor").join("exhubris");
    run_command(
        command(toolchain, &toolchain.cargo)
            .current_dir(root)
            .arg("test")
            .arg("--manifest-path")
            .arg(exhubris.join("Cargo.toml"))
            .args([
                "--locked",
                "-p",
                "hubris-build",
                "-p",
                "hubris-region-alloc",
                "--target",
                &toolchain.host,
                // Upstream's config fixtures spell booleans the KDL v1 way,
                // which the pinned KDL parser rejects.
                "--",
                "--skip",
                "config::tests",
            ]),
        "upstream build-tool tests",
    )?;

    // The CH582 PMP encoder is a self-contained file in the patched kernel;
    // compile and run its unit tests directly.
    let hubris = root.join("vendor").join("hubris");
    let pmp_tests = root.join("target").join(crate::exe("pmp-tests"));
    run_command(
        command(toolchain, &toolchain.rustc)
            .current_dir(root)
            .args(["--edition", "2021", "--test"])
            .arg(hubris.join("sys/kern/src/arch/ch582/pmp.rs"))
            .arg("-o")
            .arg(&pmp_tests),
        "PMP test build",
    )?;
    run_command(
        command(toolchain, &pmp_tests).current_dir(root),
        "PMP encoding tests",
    )?;

    println!("all checks passed.");
    Ok(())
}
