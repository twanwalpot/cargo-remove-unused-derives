use std::{
    fs, io,
    path::{Path, PathBuf},
};

use cargo_metadata::{Metadata, MetadataCommand};
use ignore::WalkBuilder;
use tempfile::TempDir;

use crate::{
    debug,
    error::{Error, Result},
    info, warn,
};

/// Path components that are never copied to the sandbox, regardless of ignore state.
const ALWAYS_EXCLUDE: &[&str] = &["target", ".git"];

/// Paths (relative to the project root) that are copied to the sandbox even if
/// hidden or gitignored.
const ALWAYS_INCLUDE: &[&str] = &["Cargo.lock", ".cargo/config.toml"];

/// The user's Rust project — never modified by the tool directly.
pub struct Project {
    metadata: Metadata,
}

/// A disposable copy of a [`Project`] in a temporary directory, where the
/// tool's destructive edits run.
pub struct Sandbox {
    metadata: Metadata,
    _tempdir: TempDir,
}

impl Project {
    pub fn detect(path: Option<PathBuf>) -> Result<Self> {
        let mut command = MetadataCommand::new();

        if let Some(path) = path {
            command.current_dir(path);
        }

        let metadata = command
            .exec()
            .map_err(|cause| Error::NotARustProject { cause })?;

        info!(
            "detected project at {}",
            metadata.workspace_root.as_std_path().display()
        );

        Ok(Self { metadata })
    }

    pub fn root(&self) -> &Path {
        self.metadata.workspace_root.as_std_path()
    }

    /// Copy each file (given as a path relative to the workspace root) from
    /// the sandbox over the corresponding file in the project.
    pub fn write_back<'a, I: IntoIterator<Item = &'a Path>>(
        &self,
        sandbox: &Sandbox,
        relative_paths: I,
    ) -> Result<()> {
        info!("writing changes back to project");
        for relative in relative_paths {
            let src = sandbox.root().join(relative);
            let dest = self.root().join(relative);

            debug!("copying {} -> {}", src.display(), dest.display());
            fs::copy(&src, &dest).map_err(|cause| Error::WriteSource { path: dest, cause })?;
        }
        Ok(())
    }

    pub fn sandbox(&self) -> Result<Sandbox> {
        info!("copying project to sandbox");

        let tempdir = copy_to_tempdir(self.metadata.workspace_root.as_std_path())
            .map_err(|cause| Error::CopySandbox { cause })?;

        debug!("sandbox at {}", tempdir.path().display());

        let metadata = MetadataCommand::new()
            .manifest_path(tempdir.path().join("Cargo.toml"))
            .exec()
            .map_err(|cause| Error::SandboxMetadata { cause })?;

        Ok(Sandbox {
            metadata,
            _tempdir: tempdir,
        })
    }
}

impl Sandbox {
    pub fn root(&self) -> &Path {
        self.metadata.workspace_root.as_std_path()
    }

    /// All `.rs` files in the sandbox's workspace packages, returned as paths
    /// relative to the workspace root. Excludes `build.rs` and any nested
    /// non-workspace `Cargo.toml` subtrees (e.g. test fixtures). If `packages`
    /// is non-empty, restricts to those package names.
    pub fn rust_files(&self, packages: &[String]) -> Vec<PathBuf> {
        let mut workspace_packages = self.metadata.workspace_packages();

        if !packages.is_empty() {
            workspace_packages.retain(|package| packages.contains(&package.name));
        }

        let mut files = Vec::new();

        for workspace_package in workspace_packages {
            let workspace_root = workspace_package.manifest_path.parent().unwrap();
            let build_rs_path = workspace_root.join("build.rs");
            let walker = WalkBuilder::new(workspace_root.as_std_path())
                .filter_entry(|entry| {
                    if entry.depth() == 0 {
                        return true;
                    }
                    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                    !(is_dir && entry.path().join("Cargo.toml").is_file())
                })
                .build();
            for entry in walker {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        warn!("skipping entry while scanning for Rust files: {err}");
                        continue;
                    }
                };
                let path = entry.path();
                let is_rust_path = path.extension().is_some_and(|e| e == "rs");
                if is_rust_path
                    && path != build_rs_path
                    && let Ok(rel) = path.strip_prefix(self.root())
                {
                    files.push(rel.to_path_buf());
                }
            }
        }

        files
    }

    /// Read a sandbox file. `path` is relative to the workspace root.
    pub fn read(&self, path: &Path) -> Result<String> {
        let absolute = self.root().join(path);
        fs::read_to_string(&absolute).map_err(|cause| Error::ReadSource {
            path: absolute,
            cause,
        })
    }

    /// Write a sandbox file. `path` is relative to the workspace root.
    pub fn write(&self, path: &Path, content: &str) -> Result<()> {
        let absolute = self.root().join(path);
        fs::write(&absolute, content).map_err(|cause| Error::WriteSource {
            path: absolute,
            cause,
        })
    }
}

/// Copy the project at `root` into a fresh temporary directory.
///
/// Respects `.gitignore` / `.ignore` and skips hidden files by default, but:
/// - always excludes `target/` and `.git/` even without any ignore file, and
/// - always includes `Cargo.lock` and `.cargo/config.toml` even when hidden
///   or gitignored, so the sandbox build matches the user's resolved graph.
fn copy_to_tempdir(root: &Path) -> io::Result<TempDir> {
    let tempdir = tempfile::tempdir()?;
    let dest = tempdir.path();

    for entry in WalkBuilder::new(root).build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                warn!("skipping entry while copying project to sandbox: {err}");
                continue;
            }
        };
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        if rel.as_os_str().is_empty() || is_always_excluded(rel) {
            continue;
        }
        let Some(ft) = entry.file_type() else {
            continue;
        };
        let dst = dest.join(rel);
        if ft.is_dir() {
            fs::create_dir_all(&dst)?;
        } else if ft.is_file() {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(path, &dst)?;
        }
    }

    for extra in ALWAYS_INCLUDE {
        let src = root.join(extra);
        if src.is_file() {
            let dst = dest.join(extra);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src, &dst)?;
        }
    }

    Ok(tempdir)
}

fn is_always_excluded(rel: &Path) -> bool {
    rel.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| ALWAYS_EXCLUDE.contains(&s))
    })
}
