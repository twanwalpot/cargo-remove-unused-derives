use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use crate::{debug, error::Result, items::Items, project::Sandbox, remove::PLACEHOLDER_MARKER};

pub fn apply_fixes(
    sandbox: &Sandbox,
    items: &Items,
    touched: HashMap<PathBuf, HashSet<usize>>,
) -> Result<()> {
    for (path, sandbox_linenos) in touched {
        let Some(items_in_path) = items.get(&path) else {
            continue;
        };

        let source = sandbox.read(&path)?;
        let mut lines: Vec<String> = source.split_inclusive('\n').map(String::from).collect();

        for item in items_in_path
            .iter()
            .filter(|i| sandbox_linenos.contains(&i.lineno_sandbox()))
        {
            let Some(marker_idx) = find_marker_line_above(&lines, item.lineno_sandbox()) else {
                debug!(
                    "no placeholder marker above {}:{}",
                    path.display(),
                    item.lineno_sandbox()
                );
                continue;
            };
            let slot_idx = marker_idx + 1;
            if slot_idx >= lines.len() {
                continue;
            }
            let restored = item.derives_restored();
            debug!(
                "restoring derive(s) {} at {}:{}",
                restored.join(", "),
                path.display(),
                item.lineno_sandbox()
            );
            let indent = line_indent(&lines[marker_idx]).to_string();
            lines[slot_idx] = build_slot_line(&indent, &lines[slot_idx], &restored);
        }

        let new_source: String = lines.concat();
        sandbox.write(&path, &new_source)?;
    }

    Ok(())
}

/// Strip placeholder markers (and their empty slot lines) from every tracked
/// sandbox file so the files are ready to be copied back to the project.
pub fn cleanup_placeholders(sandbox: &Sandbox, items: &Items) -> Result<()> {
    debug!("cleaning placeholder markers");
    for (path, _) in items.iter() {
        let source = sandbox.read(path)?;
        let cleaned = clean_placeholders(&source);
        sandbox.write(path, &cleaned)?;
    }
    Ok(())
}

/// Return the 0-based index of the nearest placeholder-marker line strictly
/// above `item_line` (1-based).
fn find_marker_line_above(lines: &[String], item_line: usize) -> Option<usize> {
    let stop = item_line.saturating_sub(1);
    (0..stop)
        .rev()
        .find(|&i| lines[i].trim_end_matches('\n').trim_start() == PLACEHOLDER_MARKER)
}

fn line_indent(line: &str) -> &str {
    let content = line.trim_end_matches('\n');
    let trimmed_len = content.trim_start().len();
    &content[..content.len() - trimmed_len]
}

/// Render the slot as `#[derive(...)]` with the given derives in source order.
/// `derives` is the full restored list for the item, so this overwrites the
/// previous slot contents rather than merging — that's how source order is
/// preserved across multiple restore passes. Preserves line termination from
/// the existing slot.
fn build_slot_line(indent: &str, existing_slot: &str, derives: &[&str]) -> String {
    let has_newline = existing_slot.ends_with('\n');
    let line = format!("{indent}#[derive({})]", derives.join(", "));
    if has_newline { line + "\n" } else { line }
}

/// Remove every placeholder marker line. If the line immediately following a
/// marker is empty (the slot was never filled, meaning no derive is needed),
/// remove that line too.
fn clean_placeholders(source: &str) -> String {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut output = String::with_capacity(source.len());
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_end_matches('\n').trim();
        if trimmed == PLACEHOLDER_MARKER {
            let drop_slot = lines
                .get(i + 1)
                .is_some_and(|l| l.trim_end_matches('\n').trim().is_empty());
            i += if drop_slot { 2 } else { 1 };
            continue;
        }
        output.push_str(lines[i]);
        i += 1;
    }
    output
}
