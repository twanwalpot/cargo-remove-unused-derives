use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    check::{Diagnostic, Location, check},
    debug,
    error::{Error, Result},
    info,
    items::{Item, Items},
    patch::{apply_fixes, cleanup_placeholders},
    project::Sandbox,
    warn,
};

const MAX_RESTORE_ATTEMPTS: usize = 64;

/// Drive the compiler-guided restore loop. In strict mode, abort if a pass
/// produces no progress or we hit `MAX_RESTORE_ATTEMPTS`, and refuse to
/// over-restore when a derive can't be pinpointed. In lenient mode, finish
/// anyway: clean up placeholders and let the caller write back whatever we
/// managed to restore.
pub fn restore(sandbox: &Sandbox, items: &mut Items, strict: bool) -> Result<()> {
    let unresolved = restore_loop(sandbox, items, strict)?;
    warn_unresolved(&unresolved);
    cleanup_placeholders(sandbox, items)
}

/// Run the compiler-guided fix loop and return whatever diagnostics couldn't
/// be resolved. An empty vec means we converged; a non-empty vec means we
/// finished in lenient mode with leftovers. Strict-mode failures bubble out
/// as `Err` and skip postprocessing in `restore`.
fn restore_loop(sandbox: &Sandbox, items: &mut Items, strict: bool) -> Result<Vec<String>> {
    let mut last_unresolved: Vec<String> = Vec::new();

    for attempt in 1..=MAX_RESTORE_ATTEMPTS {
        debug!("attempt {attempt}/{MAX_RESTORE_ATTEMPTS}");

        let diagnostics = check(sandbox.root())?;

        if diagnostics.is_empty() {
            info!("restore converged after {attempt} attempt(s)");
            return Ok(Vec::new());
        }

        let FixOutcome {
            unresolved,
            progressed,
        } = fix_diagnostics(sandbox, items, &diagnostics, strict)?;

        if !progressed {
            if strict {
                return Err(Error::RestoreNoProgress { unresolved });
            }
            return Ok(unresolved);
        }

        last_unresolved = unresolved;
    }

    if strict {
        Err(Error::UnableToRestore {
            max_attempts: MAX_RESTORE_ATTEMPTS,
            unresolved: last_unresolved,
        })
    } else {
        Ok(last_unresolved)
    }
}

fn warn_unresolved(unresolved: &[String]) {
    if unresolved.is_empty() {
        return;
    }
    warn!(
        "{} diagnostic(s) could not be resolved; finishing anyway",
        unresolved.len()
    );
    for msg in unresolved {
        warn!("  - {msg}");
    }
}

/// Outcome of one `fix_diagnostics` pass. `progressed` is true iff at least
/// one slot was updated. `unresolved` lists diagnostics we couldn't act on,
/// either because no item matched or because we couldn't pick which derive
/// to restore in strict mode.
struct FixOutcome {
    unresolved: Vec<String>,
    progressed: bool,
}

fn fix_diagnostics(
    sandbox: &Sandbox,
    items: &mut Items,
    diagnostics: &[Diagnostic],
    strict: bool,
) -> Result<FixOutcome> {
    info!("attempting to fix diagnostics ({})", diagnostics.len());

    let mut unresolved = Vec::<String>::new();
    // Per file, the sandbox line of every item whose slot needs to be rewritten
    // this pass. apply_fixes looks each item up in `items` and re-renders its
    // slot from `derives_restored()` — i.e. in the user's original source order.
    let mut touched = HashMap::<PathBuf, HashSet<usize>>::new();

    for diagnostic in diagnostics {
        let Some((path, item)) = find_item_for_diagnostic(sandbox, items, diagnostic) else {
            debug!("no matching item for diagnostic: {}", diagnostic.message());
            unresolved.push(diagnostic.message().to_string());
            continue;
        };

        // With a single pending derive there's nothing to disambiguate;
        // skip the pinpoint pass entirely. Otherwise try to identify the
        // exact missing one from the diagnostic text. If we can't, lenient
        // mode over-restores; strict mode leaves it as unresolved so the
        // outer loop will eventually fail.
        let derives_to_restore: Vec<String> = if item.derives_unused().len() == 1 {
            item.derives_unused().to_vec()
        } else if let Some(d) = diagnostic.references_derive(item.derives_unused()) {
            vec![d.to_string()]
        } else if strict {
            debug!(
                "could not pinpoint the missing derive for `{}`; refusing to over-restore in strict mode",
                item.name()
            );
            unresolved.push(format!(
                "could not pinpoint missing derive for `{}` (pending: {}): {}",
                item.name(),
                item.derives_unused().join(", "),
                diagnostic.message(),
            ));
            continue;
        } else {
            let pending = item.derives_unused().to_vec();
            warn!(
                "could not pinpoint the missing derive for `{}`; restoring all of [{}] \
                 — some of these may not actually be needed",
                item.name(),
                pending.join(", ")
            );
            pending
        };

        for d in &derives_to_restore {
            item.mark_restored(d);
        }
        let lineno = item.lineno_sandbox();
        let path = path.to_path_buf();

        touched.entry(path).or_default().insert(lineno);
    }

    let progressed = !touched.is_empty();
    apply_fixes(sandbox, items, touched)?;

    Ok(FixOutcome {
        unresolved,
        progressed,
    })
}

/// Find the item a diagnostic is tied to by trying three strategies in order:
/// name-from-text, name-from-source-peek, then span proximity. The caller
/// then either pinpoints the missing derive or restores all of the item's
/// pending derives.
fn find_item_for_diagnostic<'a>(
    sandbox: &Sandbox,
    items: &'a mut Items,
    diagnostic: &Diagnostic,
) -> Option<(&'a Path, &'a mut Item)> {
    let (pick_path, idx) = pick_by_text_names(items, diagnostic)
        .or_else(|| pick_by_source_peek(sandbox, items, diagnostic))
        .or_else(|| pick_by_span_proximity(items, diagnostic))?;

    let (path, item) = items.get_mut_at(&pick_path, idx)?;

    debug!(
        "matched `{}` at {} to: {}",
        item.name(),
        path.display(),
        diagnostic.message()
    );

    Some((path, item))
}

/// Pick by type-name candidates pulled from backticks in the diagnostic text.
fn pick_by_text_names(items: &Items, diagnostic: &Diagnostic) -> Option<(PathBuf, usize)> {
    let candidates = diagnostic.item_names();
    if candidates.is_empty() {
        return None;
    }
    pick_by_name(items, diagnostic, &candidates)
}

/// Pick by type-name candidates extracted from the source line(s) at the
/// diagnostic's location. Rescues axum-style errors where
/// `#[diagnostic::on_unimplemented]` strips the inner trait bound (e.g. the
/// diagnostic mentions `Handler<S>` but not `HandlerParams`, while the source
/// line is `Query<HandlerParams>`).
fn pick_by_source_peek(
    sandbox: &Sandbox,
    items: &Items,
    diagnostic: &Diagnostic,
) -> Option<(PathBuf, usize)> {
    let loc = diagnostic.location()?;
    let candidates = source_peek_candidates(sandbox, loc);
    if candidates.is_empty() {
        return None;
    }
    pick_by_name(items, diagnostic, &candidates)
}

/// Fallback: the pending item whose definition line is closest to the
/// diagnostic's location in the same file. A guess, not a certainty — which
/// is why it runs only after the name strategies fail.
fn pick_by_span_proximity(items: &Items, diagnostic: &Diagnostic) -> Option<(PathBuf, usize)> {
    let loc = diagnostic.location()?;
    let span_line = loc.line_start as i64;
    let mut min_distance = u64::MAX;
    let mut best = None;
    for (path, items_in_path) in items.iter() {
        if loc.file != path {
            continue;
        }
        for (idx, item) in items_in_path.iter().enumerate() {
            if item.derives_unused().is_empty() {
                continue;
            }
            let distance = (item.lineno_sandbox() as i64 - span_line).unsigned_abs();
            if distance < min_distance {
                min_distance = distance;
                best = Some((path.to_path_buf(), idx));
            }
        }
    }
    best
}

/// Score each item whose name matches a candidate and return the best hit.
/// Items with no pending derives can't be the source of a derive error and
/// would otherwise outrank the real culprit when they happen to live in the
/// diagnostic's span file.
fn pick_by_name(
    items: &Items,
    diagnostic: &Diagnostic,
    candidates: &[String],
) -> Option<(PathBuf, usize)> {
    let mut best: Option<(PathBuf, usize, u8)> = None;
    for (path, items_in_path) in items.iter() {
        let path_in_spans = diagnostic.references_file(path);
        for (idx, item) in items_in_path.iter().enumerate() {
            if item.derives_unused().is_empty() {
                continue;
            }
            if !candidates.iter().any(|c| c == item.name()) {
                continue;
            }
            let derive_match = diagnostic
                .references_derive(item.derives_unused())
                .is_some();
            let score = (if derive_match { 4 } else { 0 }) + (if path_in_spans { 2 } else { 0 });
            if best.as_ref().is_none_or(|(_, _, s)| score > *s) {
                best = Some((path.to_path_buf(), idx, score));
            }
        }
    }
    best.map(|(p, i, _)| (p, i))
}

/// Type-name candidates extracted from the source line(s) at `loc`. Picks
/// every uppercase-leading identifier on those lines — generic args
/// (`Query<HandlerParams>`) and qualified paths (`crate::module::Foo`) split
/// naturally on non-identifier chars. Cheap and degrades to no candidates
/// when the file can't be read.
fn source_peek_candidates(sandbox: &Sandbox, loc: Location) -> Vec<String> {
    let Ok(source) = sandbox.read(loc.file) else {
        return Vec::new();
    };

    let lines: Vec<&str> = source.lines().collect();
    let start = loc.line_start.saturating_sub(1);
    let end = loc.line_end.min(lines.len());
    if start >= end {
        return Vec::new();
    }

    let mut result = Vec::new();
    for line in &lines[start..end] {
        for ident in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if ident.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                result.push(ident.to_string());
            }
        }
    }
    result
}
