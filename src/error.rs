use std::{fmt, io, path::PathBuf};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    Dirty,
    NoVcs,
    NotARustProject {
        cause: cargo_metadata::Error,
    },
    CopySandbox {
        cause: io::Error,
    },
    SandboxMetadata {
        cause: cargo_metadata::Error,
    },
    ReadSource {
        path: PathBuf,
        cause: io::Error,
    },
    WriteSource {
        path: PathBuf,
        cause: io::Error,
    },
    ParseSource {
        path: PathBuf,
        cause: syn::Error,
    },
    CargoCheck {
        cause: io::Error,
    },
    ParseCargoMessage {
        cause: io::Error,
    },
    UnableToRestore {
        max_attempts: usize,
        unresolved: Vec<String>,
    },
    RestoreNoProgress {
        unresolved: Vec<String>,
    },
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Dirty | Error::NoVcs | Error::NotARustProject { .. } => 2,
            _ => 1,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Dirty => f.write_str(
                "the working directory of this package has uncommitted changes; if you'd like to suppress this error pass `--allow-dirty`, or commit your changes",
            ),
            Error::NoVcs => f.write_str(
                "you are not in a git repository; if you'd like to suppress this error pass `--allow-no-vcs`",
            ),
            Error::NotARustProject { cause } => write!(
                f,
                "could not read cargo metadata from the current directory; is this a Rust project? ({cause})",
            ),
            Error::CopySandbox { cause } => {
                write!(f, "failed to copy the project into a sandbox: {cause}")
            }
            Error::SandboxMetadata { cause } => write!(
                f,
                "failed to read cargo metadata from the sandbox copy: {cause}"
            ),
            Error::ReadSource { path, cause } => {
                write!(f, "failed to read {}: {cause}", path.display())
            }
            Error::WriteSource { path, cause } => {
                write!(f, "failed to write {}: {cause}", path.display())
            }
            Error::ParseSource { path, cause } => {
                write!(f, "failed to parse {}: {cause}", path.display())
            }
            Error::CargoCheck { cause } => {
                write!(f, "failed to run `cargo check`: {cause}")
            }
            Error::ParseCargoMessage { cause } => {
                write!(f, "failed to parse cargo message output: {cause}")
            }
            Error::UnableToRestore {
                max_attempts,
                unresolved,
            } => {
                writeln!(
                    f,
                    "unable to restore the touched files; max restore attempts exceeded ({max_attempts})"
                )?;
                if !unresolved.is_empty() {
                    writeln!(
                        f,
                        "{} diagnostic(s) from the last attempt could not be resolved:",
                        unresolved.len()
                    )?;
                    for msg in unresolved {
                        writeln!(f, "  - {msg}")?;
                    }
                }
                Ok(())
            }
            Error::RestoreNoProgress { unresolved } => {
                writeln!(
                    f,
                    "unable to restore the touched files; no derives could be restored for the reported diagnostics, so re-running `cargo check` would produce the same output"
                )?;
                if !unresolved.is_empty() {
                    writeln!(
                        f,
                        "{} diagnostic(s) could not be resolved:",
                        unresolved.len()
                    )?;
                    for msg in unresolved {
                        writeln!(f, "  - {msg}")?;
                    }
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Dirty
            | Error::NoVcs
            | Error::UnableToRestore { .. }
            | Error::RestoreNoProgress { .. } => None,
            Error::NotARustProject { cause } | Error::SandboxMetadata { cause } => Some(cause),
            Error::CopySandbox { cause }
            | Error::ReadSource { cause, .. }
            | Error::WriteSource { cause, .. }
            | Error::CargoCheck { cause }
            | Error::ParseCargoMessage { cause } => Some(cause),
            Error::ParseSource { cause, .. } => Some(cause),
        }
    }
}
