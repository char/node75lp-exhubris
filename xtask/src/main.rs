mod build;
mod test;
mod upstream;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Build and test the Node75 LP exhubris firmware")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Clone pinned upstreams into ignored vendor/ and apply patches/
    Fetch {
        /// Rebuild the vendor checkouts even if they already match the pins
        #[arg(long)]
        force: bool,
    },
    /// Remove the generated vendor/ checkouts
    CleanUpstreams,
    /// Build a raw .bin (never accesses hardware)
    Build,
    /// Build the firmware image and run the host/upstream test suites
    Test,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    // `cargo xtask` builds this from the workspace root's xtask/ directory.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root")
        .to_path_buf();
    match cli.command {
        Cmd::Fetch { force } => {
            upstream::ensure_upstreams(&root, force)?;
            println!(
                "Generated upstream checkouts are ready in vendor/ (ignored, not checked in)."
            );
        }
        Cmd::CleanUpstreams => {
            for name in ["exhubris", "hubris"] {
                let dir = root.join("vendor").join(name);
                if dir.exists() {
                    fs::remove_dir_all(&dir)?;
                }
            }
            println!("Removed generated vendor/exhubris/ and vendor/hubris/");
        }
        Cmd::Build => {
            let toolchain = Toolchain::find()?;
            build::build(&root, &toolchain)?;
        }
        Cmd::Test => {
            let toolchain = Toolchain::find()?;
            build::build(&root, &toolchain)?;
            test::test(&root, &toolchain)?;
        }
    }
    Ok(())
}

pub fn run_command(command: &mut Command, what: &str) -> Result<()> {
    eprintln!("+ {}", command_line(command));
    let status = command
        .status()
        .map_err(|e| anyhow!("failed to start {what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{what} failed with {status}"))
    }
}

pub fn capture(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(anyhow!("command failed with {}", output.status));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn command_line(command: &Command) -> String {
    let mut parts = vec![command.get_program().to_string_lossy().into_owned()];
    parts.extend(
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned()),
    );
    parts.join(" ")
}

pub fn git() -> Command {
    Command::new("git")
}

pub fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// The pinned toolchain's real executables, bypassing rustup/cargo shims.
/// The PoC's build environment misbehaved through the shims (a host mount
/// point issue), so builds invoke the toolchain directory directly.
pub struct Toolchain {
    cargo: PathBuf,
    rustc: PathBuf,
    rustfmt: PathBuf,
    objcopy: PathBuf,
    host: String,
}

impl Toolchain {
    fn find() -> Result<Self> {
        // Cargo resolves rust-toolchain.toml and names its own real binary here.
        let cargo = PathBuf::from(env::var_os("CARGO").context("run xtask via `cargo xtask`")?);
        let bin = cargo.parent().context("CARGO has no parent directory")?;
        let rustc = bin.join(exe("rustc"));
        let printed =
            capture(Command::new(&rustc).args(["--print", "host-tuple", "--print", "sysroot"]))?;
        let Some((host, sysroot)) = printed.split_once('\n') else {
            return Err(anyhow!("unexpected `rustc --print` output: {printed:?}"));
        };
        let host = host.trim().to_owned();
        Ok(Self {
            objcopy: Path::new(sysroot.trim())
                .join("lib/rustlib")
                .join(&host)
                .join("bin")
                .join(exe("llvm-objcopy")),
            rustfmt: bin.join(exe("rustfmt")),
            rustc,
            cargo,
            host,
        })
    }
}

pub fn command(toolchain: &Toolchain, program: impl AsRef<Path>) -> Command {
    let mut command = Command::new(program.as_ref());
    let bin = toolchain
        .cargo
        .parent()
        .expect("toolchain cargo has a parent")
        .to_path_buf();
    let separator = if cfg!(windows) { ";" } else { ":" };
    let path = env::var_os("PATH")
        .map(|old| {
            format!(
                "{}{separator}{}",
                bin.display(),
                PathBuf::from(old).display()
            )
        })
        .unwrap_or_else(|| bin.display().to_string());
    command
        .env("PATH", path)
        .env("RUSTC", &toolchain.rustc)
        .env("RUSTFMT", &toolchain.rustfmt)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR");
    command
}
