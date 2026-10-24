use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Result, anyhow};

use crate::{capture, git, run_command};

// (url, rev). The hubris rev is the one exhubris's Cargo.lock pins (branch
// cbiffle/exhubris-fixes); bump them together.
const EXHUBRIS: (&str, &str) = (
    "https://github.com/cbiffle/exhubris",
    "64daa2d029cfd9bfabc0a61946784a9279ba478e",
);
const HUBRIS: (&str, &str) = (
    "https://github.com/cbiffle/hubris-fork",
    "061df2fc76433bbc7cc3931a8981fbd509e35c97",
);

pub fn ensure_upstreams(root: &Path, force: bool) -> Result<()> {
    ensure_upstream(root, "exhubris", EXHUBRIS, force)?;
    ensure_upstream(root, "hubris", HUBRIS, force)?;
    Ok(())
}

fn patch_series(root: &Path, name: &str) -> Result<Vec<PathBuf>> {
    let dir = root.join("patches").join(name);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => {
            return Err(anyhow!(
                "cannot read patch series {}: {error}",
                dir.display()
            ));
        }
    };
    let mut series: Vec<PathBuf> = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "patch"))
        .collect();
    series.sort();
    Ok(series)
}

fn ensure_upstream(
    root: &Path,
    name: &str,
    (url, rev): (&str, &str),
    force: bool,
) -> Result<PathBuf> {
    let dest = root.join("vendor").join(name);
    let series = patch_series(root, name)?;
    let stamp = series
        .iter()
        .map(fs::read)
        .collect::<std::io::Result<Vec<_>>>()?
        .concat();
    let stamp_path = dest.join(".git/node75-patches");
    if !force
        && fs::read(&stamp_path).ok().as_deref() == Some(stamp.as_slice())
        && patched_checkout_matches(&dest, &series, rev)
    {
        println!("{name} already at {rev} with patches applied");
        return Ok(dest);
    }
    if dest.is_dir() {
        fs::remove_dir_all(&dest)?;
    } else if dest.exists() {
        fs::remove_file(&dest)?;
    }
    fs::create_dir_all(dest.parent().expect("upstream cache has a parent"))?;

    let dest_arg = dest.to_string_lossy();
    run_command(git().arg("init").arg(&dest), "git init")?;
    run_command(
        git().args(["-C", &dest_arg, "config", "core.autocrlf", "false"]),
        "git config",
    )?;
    run_command(
        git().args(["-C", &dest_arg, "remote", "add", "origin", url]),
        "git remote add",
    )?;
    run_command(
        git().args(["-C", &dest_arg, "fetch", "--depth", "1", "origin", rev]),
        "git fetch",
    )?;
    run_command(
        git().args(["-C", &dest_arg, "checkout", "--detach", "FETCH_HEAD"]),
        "git checkout",
    )?;
    for patch in &series {
        run_command(
            git()
                .args(["-C", &dest_arg, "apply", "--whitespace=nowarn"])
                .arg(patch),
            "git apply",
        )?;
    }
    fs::write(stamp_path, stamp)?;
    println!("Prepared {name} at {rev} with {} patches", series.len());
    Ok(dest)
}

fn patched_checkout_matches(dest: &Path, series: &[PathBuf], rev: &str) -> bool {
    let dest_arg = dest.to_string_lossy();
    let head_matches = capture(git().args(["-C", &dest_arg, "rev-parse", "HEAD"]))
        .map(|head| head == rev)
        .unwrap_or(false);
    // The series is file-disjoint, so each patch can be reverse-checked
    // independently against the fully patched tree.
    let applied = series.iter().all(|patch| {
        git()
            .args(["-C", &dest_arg, "apply", "--reverse", "--check"])
            .arg(patch)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    });
    head_matches && applied
}
