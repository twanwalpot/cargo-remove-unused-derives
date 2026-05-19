use std::{
    collections::HashSet,
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

use cargo_metadata::{Message, diagnostic::DiagnosticLevel};

use crate::{
    debug,
    error::{Error, Result},
    info,
};

/// A rustc error diagnostic captured from `cargo check`, paired with its
/// flattened text form so name-matching and derive-matching don't have to
/// re-walk the diagnostic tree on every query.
pub struct Diagnostic {
    inner: cargo_metadata::diagnostic::Diagnostic,
    text: String,
}

/// The source location a diagnostic points at — file plus line range — drawn
/// from the primary span (or first span as fallback).
pub struct Location<'a> {
    pub file: &'a Path,
    pub line_start: usize,
    pub line_end: usize,
}

impl Diagnostic {
    fn new(inner: cargo_metadata::diagnostic::Diagnostic) -> Self {
        let text = flatten(&inner);
        Self { inner, text }
    }

    pub fn message(&self) -> &str {
        &self.inner.message
    }

    /// The source location this diagnostic points at: the file and line range
    /// of the primary span (or the first span as a defensive fallback).
    pub fn location(&self) -> Option<Location<'_>> {
        let span = self
            .inner
            .spans
            .iter()
            .find(|s| s.is_primary)
            .or(self.inner.spans.first())?;

        Some(Location {
            file: Path::new(&span.file_name),
            line_start: span.line_start,
            line_end: span.line_end,
        })
    }

    /// True if `path` matches any file referenced by this diagnostic, including
    /// child note spans. Both sides are workspace-relative (cargo's absolute
    /// paths are stripped to relative at construction), so `==` is enough.
    pub fn references_file(&self, path: &Path) -> bool {
        self.inner
            .spans
            .iter()
            .any(|s| Path::new(&s.file_name) == path)
            || self
                .inner
                .children
                .iter()
                .any(|c| c.spans.iter().any(|s| Path::new(&s.file_name) == path))
    }

    /// Type-name candidates referenced by this diagnostic. Each backtick-quoted
    /// region is split into identifier paths; for each path we take the final
    /// `::` segment with generic args and trait-bound colons stripped, and keep
    /// the ones that look like type names (i.e. start uppercase).
    pub fn item_names(&self) -> Vec<String> {
        extract_item_names_in_text(&self.text)
    }

    /// First derive from `derives` referenced by this diagnostic, if any.
    /// Scans the flattened text — message, span labels, children's messages
    /// and labels, and any `suggested_replacement` — for a mention of each
    /// derive in turn.
    pub fn references_derive<'a>(&self, derives: &'a [String]) -> Option<&'a str> {
        derives
            .iter()
            .find(|d| derive_mentioned_in(d, &self.text))
            .map(String::as_str)
    }
}

/// Run `cargo check` in the sandbox and collect the compiler diagnostics.
/// Deduplicates: `--all-targets` re-emits the same error once per target
/// (e.g. lib + lib's test binary), and we only want to act on each once.
pub fn check(root: &Path) -> Result<Vec<Diagnostic>> {
    info!("running cargo check");

    let started = Instant::now();

    let output = Command::new("cargo")
        .args([
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json",
        ])
        .current_dir(root)
        .stderr(Stdio::null())
        .output()
        .map_err(|cause| Error::CargoCheck { cause })?;

    let mut diagnostics = Vec::new();
    let mut seen = HashSet::<(String, Option<(String, u32)>)>::new();

    for msg in Message::parse_stream(output.stdout.as_slice()) {
        let msg = msg.map_err(|cause| Error::ParseCargoMessage { cause })?;
        if let Message::CompilerMessage(m) = msg
            && matches!(m.message.level, DiagnosticLevel::Error)
        {
            let mut diag = m.message;
            normalize_span_paths(&mut diag, root);
            let primary = diag
                .spans
                .iter()
                .find(|s| s.is_primary)
                .or(diag.spans.first())
                .map(|s| (s.file_name.clone(), s.byte_start));
            if seen.insert((diag.message.clone(), primary)) {
                diagnostics.push(Diagnostic::new(diag));
            }
        }
    }

    debug!(
        "produced {} diagnostic(s) in {:.2}s",
        diagnostics.len(),
        started.elapsed().as_secs_f64()
    );

    Ok(diagnostics)
}

/// Flatten a cargo diagnostic into a single text blob: top-level message + span
/// labels, plus every child's message, span labels, and suggested replacements.
fn flatten(diagnostic: &cargo_metadata::diagnostic::Diagnostic) -> String {
    let mut text = diagnostic.message.clone();
    for span in &diagnostic.spans {
        if let Some(label) = &span.label {
            text.push('\n');
            text.push_str(label);
        }
    }
    for child in &diagnostic.children {
        text.push('\n');
        text.push_str(&child.message);
        for span in &child.spans {
            if let Some(label) = &span.label {
                text.push('\n');
                text.push_str(label);
            }
            if let Some(replacement) = &span.suggested_replacement {
                text.push('\n');
                text.push_str(replacement);
            }
        }
    }
    text
}

fn extract_item_names_in_text(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    for fragment in backtick_fragments(text) {
        for path in split_paths(fragment) {
            let last = path.rsplit("::").next().unwrap_or(path);
            let base = last.split('<').next().unwrap_or(last);
            let cleaned = base.trim_matches(':');
            if cleaned
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
            {
                result.push(cleaned.to_string());
            }
        }
    }
    result
}

fn backtick_fragments(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let open = rest.find('`')?;
        rest = &rest[open + 1..];
        let close = rest.find('`')?;
        let fragment = &rest[..close];
        rest = &rest[close + 1..];
        Some(fragment)
    })
}

fn derive_mentioned_in(derive: &str, text: &str) -> bool {
    let name = derive.rsplit("::").next().unwrap_or(derive);
    mentioned_in_backticks(name, text) || mentioned_in_derive_list(name, text)
}

/// True if any backtick-quoted region in `text` contains a Rust path whose
/// final `::` segment (with generic args stripped) equals `name`. A single
/// backtick region can contain a bare name, a qualified path, a trait bound
/// (`Type: Trait`), or multi-trait bounds with generics — all are handled.
fn mentioned_in_backticks(name: &str, text: &str) -> bool {
    for fragment in backtick_fragments(text) {
        for path in split_paths(fragment) {
            let last = path.rsplit("::").next().unwrap_or(path);
            let base = last.split('<').next().unwrap_or(last);
            if base == name {
                return true;
            }
        }
    }
    false
}

/// Split `s` into identifier-path runs (identifiers joined by `::`), treating
/// anything else (spaces, `<`, `>`, `+`, `,`, single `:` in trait bounds,
/// parens, lifetimes, etc.) as a separator.
fn split_paths(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
        .filter(|p| !p.is_empty())
}

/// Rewrite every span's `file_name` to workspace-relative if it starts with
/// `root`. cargo emits diagnostic file names as a mix of workspace-relative
/// and absolute; normalizing here means downstream code can compare paths
/// with `==` rather than a fuzzy suffix match.
fn normalize_span_paths(diag: &mut cargo_metadata::diagnostic::Diagnostic, root: &Path) {
    for span in &mut diag.spans {
        relativize(&mut span.file_name, root);
    }
    for child in &mut diag.children {
        for span in &mut child.spans {
            relativize(&mut span.file_name, root);
        }
    }
}

fn relativize(file_name: &mut String, root: &Path) {
    if let Ok(rel) = Path::new(file_name.as_str()).strip_prefix(root) {
        *file_name = rel.to_string_lossy().into_owned();
    }
}

/// True if `name` appears as an element of any `derive(...)` list in `text`.
fn mentioned_in_derive_list(name: &str, text: &str) -> bool {
    let mut rest = text;
    loop {
        let Some(idx) = rest.find("derive(") else {
            return false;
        };
        rest = &rest[idx + "derive(".len()..];
        let Some(close) = rest.find(')') else {
            return false;
        };
        let list = &rest[..close];
        if list.split(',').map(str::trim).any(|d| d == name) {
            return true;
        }
        rest = &rest[close + 1..];
    }
}

#[cfg(test)]
mod tests {
    use super::{derive_mentioned_in, extract_item_names_in_text};

    #[test]
    fn candidate_names_from_doesnt_implement_phrase() {
        let cands =
            extract_item_names_in_text("`ReorderError` doesn't implement `std::fmt::Debug`");
        assert!(cands.iter().any(|c| c == "ReorderError"));
    }

    #[test]
    fn candidate_names_from_trait_bound_with_colon() {
        let cands = extract_item_names_in_text(
            "the trait bound `convert_core::helpers::reorder::Placement: serde::Deserialize<'de>` is not satisfied",
        );
        assert!(cands.iter().any(|c| c == "Placement"));
    }

    #[test]
    fn candidate_names_strip_leading_reference() {
        let cands =
            extract_item_names_in_text("binary operation `==` cannot be applied to type `&Color`");
        assert!(cands.iter().any(|c| c == "Color"));
    }

    #[test]
    fn candidate_names_from_has_type_phrase() {
        let cands =
            extract_item_names_in_text("cannot move out of `foo.field` which has type `Box<Op>`");
        assert!(cands.iter().any(|c| c == "Op"));
    }

    #[test]
    fn matches_bare_name_in_backticks() {
        assert!(derive_mentioned_in(
            "Default",
            "the trait `Default` is not impl"
        ));
    }

    #[test]
    fn matches_qualified_path_in_backticks() {
        assert!(derive_mentioned_in(
            "Default",
            "candidate #1: `std::default::Default`"
        ));
    }

    #[test]
    fn matches_inside_derive_list() {
        assert!(derive_mentioned_in(
            "Debug",
            "add `#[derive(Clone, Debug)]`"
        ));
    }

    #[test]
    fn ignores_unrelated_similar_names() {
        assert!(!derive_mentioned_in(
            "Default",
            "the trait `NonDefault` is not impl"
        ));
        assert!(!derive_mentioned_in("Default", "not in backticks: Default"));
    }

    #[test]
    fn last_segment_of_qualified_derive_is_used() {
        // Tool tracks derives as e.g. "serde::Serialize"; the diagnostic may
        // reference `serde::Serialize` or just `Serialize`.
        assert!(derive_mentioned_in(
            "serde::Serialize",
            "the trait `serde::Serialize` is not impl"
        ));
        assert!(derive_mentioned_in(
            "serde::Serialize",
            "the trait `Serialize` is not impl"
        ));
    }

    #[test]
    fn matches_trait_with_generic_args_in_bound() {
        // Seen in the wild: `the trait bound `User: serde::Deserialize<'de>` is not satisfied`.
        assert!(derive_mentioned_in(
            "Deserialize",
            "the trait bound `User: serde::Deserialize<'de>` is not satisfied"
        ));
    }

    #[test]
    fn matches_trait_in_multi_trait_bound() {
        assert!(derive_mentioned_in(
            "Debug",
            "the trait bound `Foo: Clone + Debug` is not satisfied"
        ));
    }
}
