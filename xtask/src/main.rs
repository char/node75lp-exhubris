mod upstream;

use std::{
    env, fs,
    path::PathBuf,
    process::{Command, ExitCode},
};

use anyhow::{Result, anyhow};
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
