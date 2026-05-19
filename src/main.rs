mod check;
mod error;
mod items;
mod patch;
mod project;
mod remove;
mod report;
mod restore;

use std::{
    path::{Path, PathBuf},
    process::{Command, exit},
    sync::atomic::{AtomicBool, Ordering},
};

use clap::Parser;

use crate::{
    error::{Error, Result},
    project::Project,
};

pub static VERBOSE: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(
    name = "cargo-remove-unused-derives",
    bin_name = "cargo remove-unused-derives",
    version,
    about
)]
struct Cli {
    /// The path from where to detect the project.
    path: Option<PathBuf>,
    /// Only process the given package(s).
    #[arg(short, long = "package", value_name = "NAME")]
    packages: Vec<String>,
    /// Actually remove unused derives from the files. If omitted, your files will not be touched.
    #[arg(long)]
    write: bool,
    /// Proceed even if the git working tree has uncommitted changes.
    #[arg(long)]
    allow_dirty: bool,
    /// Proceed even if you are not in a git working tree.
    #[arg(long)]
    allow_no_vcs: bool,
    /// Fail if any unused derive can't be confidently identified. By default,
    /// the tool over-restores rather than failing when it can't pinpoint which
    /// of an item's derives is needed.
    #[arg(long)]
    strict: bool,
    /// Print progress output to stderr.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum CargoCli {
    RemoveUnusedDerives(Cli),
}

fn main() {
    let CargoCli::RemoveUnusedDerives(cli) = CargoCli::parse();
    VERBOSE.store(cli.verbose, Ordering::Relaxed);

    if let Err(err) = run(cli) {
        eprintln!("error: {err}");
        exit(err.exit_code());
    }
}

fn run(cli: Cli) -> Result<()> {
    if cli.write {
        let in_repo = in_git_repo();

        if !in_repo && !cli.allow_no_vcs {
            return Err(Error::NoVcs);
        }

        if in_repo && !cli.allow_dirty && is_git_repo_dirty() {
            return Err(Error::Dirty);
        }
    }

    let project = Project::detect(cli.path)?;
    let sandbox = project.sandbox()?;

    let mut items = remove::remove(&sandbox, &cli.packages)?;
    restore::restore(&sandbox, &mut items, cli.strict)?;

    if cli.write {
        let touched: Vec<&Path> = items.iter().map(|(p, _)| p).collect();
        project.write_back(&sandbox, touched)?;
    }

    items.sort();

    print!("{items}");

    Ok(())
}

fn in_git_repo() -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_git_repo_dirty() -> bool {
    !Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.status.success() && o.stdout.is_empty())
        .unwrap_or(false)
}

/// Info-level log to stderr; always shown.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        eprintln!("info: {}", format_args!($($arg)*));
    };
}

/// Warn-level log to stderr; always shown.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        eprintln!("warn: {}", format_args!($($arg)*));
    };
}

/// Debug-level log to stderr; only shown when `--verbose` was set.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::VERBOSE.load(::std::sync::atomic::Ordering::Relaxed) {
            eprintln!("debug: {}", format_args!($($arg)*));
        }
    };
}
