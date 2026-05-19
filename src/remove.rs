use std::{ops::Range, path::Path};

use syn::{Token, punctuated::Punctuated, spanned::Spanned};

use crate::{
    debug,
    error::{Error, Result},
    info,
    items::{Item, Items},
    project::Sandbox,
};

/// Marker comment left in place of a stripped `#[derive(...)]` attribute.
/// Restore expects a 2-line block: this marker on one line and an empty
/// "slot" line immediately below that the compiler-driven restore pass writes
/// `#[derive(...)]` into.
pub(crate) const PLACEHOLDER_MARKER: &str = "// __DERIVE_PLACEHOLDER__";

struct Extracted {
    name: String,
    line: usize,
    attrs: Vec<DeriveAttr>,
}

/// A single `#[derive(...)]` attribute in the source.
struct DeriveAttr {
    typepaths: Vec<String>,
    /// Byte range of the entire `#[derive(...)]` in the original source.
    byte_range: Range<usize>,
    /// Line range of the entire `#[derive(...)]` in the original source.
    line_range: Range<usize>,
}

pub fn remove(sandbox: &Sandbox, packages: &[String]) -> Result<Items> {
    info!("stripping derives in sandbox");

    let mut items = Items::default();

    for file in sandbox.rust_files(packages) {
        debug!("stripping {}", file.display());
        let items_in_file = remove_derives(sandbox, &file)?;
        items.insert(file, items_in_file);
    }

    debug!("stripped derives from {} file(s)", items.len());
    Ok(items)
}

fn remove_derives(sandbox: &Sandbox, path: &Path) -> Result<Vec<Item>> {
    let mut source = sandbox.read(path)?;
    let items = remove_derives_from_source(&mut source).map_err(|cause| Error::ParseSource {
        path: path.to_path_buf(),
        cause,
    })?;
    sandbox.write(path, &source)?;
    Ok(items)
}

fn remove_derives_from_source(source: &mut String) -> syn::Result<Vec<Item>> {
    Ok(edit_source(source, extract_items(source)?))
}

fn edit_source(source: &mut String, extracted: Vec<Extracted>) -> Vec<Item> {
    let substitution = format!("{PLACEHOLDER_MARKER}\n");

    let mut byte_shift: isize = 0;
    let mut line_shift: isize = 0;

    extracted
        .into_iter()
        .map(|ext| {
            let lineno_source = ext.line;

            for attr in &ext.attrs {
                let start = (attr.byte_range.start as isize + byte_shift) as usize;
                let end = (attr.byte_range.end as isize + byte_shift) as usize;
                source.replace_range(start..end, &substitution);

                let attr_bytes = (attr.byte_range.end - attr.byte_range.start) as isize;
                byte_shift += substitution.len() as isize - attr_bytes;

                let attr_lines = (attr.line_range.end - attr.line_range.start) as isize + 1;
                line_shift += 2 - attr_lines;
            }

            let lineno_sandbox = lineno_source as isize + line_shift;

            Item::new(
                ext.name,
                lineno_source,
                lineno_sandbox as usize,
                ext.attrs
                    .into_iter()
                    .flat_map(|attr| attr.typepaths)
                    .collect(),
            )
        })
        .collect()
}

fn extract_items(source: &str) -> syn::Result<Vec<Extracted>> {
    syn::parse_file(source)?
        .items
        .iter()
        .map(try_parse_item)
        .filter_map(Result::transpose)
        .collect()
}

fn try_parse_item(item: &syn::Item) -> syn::Result<Option<Extracted>> {
    let (ident, attrs) = match item {
        syn::Item::Struct(s) => (&s.ident, &s.attrs),
        syn::Item::Enum(e) => (&e.ident, &e.attrs),
        _ => return Ok(None),
    };

    Ok(try_parse_derives(attrs)?.map(|derive_attrs| Extracted {
        name: ident.to_string(),
        line: ident.span().start().line,
        attrs: derive_attrs,
    }))
}

fn try_parse_derives(attrs: &[syn::Attribute]) -> syn::Result<Option<Vec<DeriveAttr>>> {
    let derive_attrs = attrs.iter().filter(|attr| attr.path().is_ident("derive"));

    let mut result = Vec::new();

    for attr in derive_attrs {
        let mut typepaths = Vec::new();

        let byte_range = attr.span().byte_range();
        let line_range = attr.span().start().line..attr.span().end().line;

        let paths = attr.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)?;

        for p in paths {
            typepaths.push(
                p.segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::"),
            );
        }

        result.push(DeriveAttr {
            typepaths,
            byte_range,
            line_range,
        });
    }

    Ok(if result.is_empty() {
        None
    } else {
        Some(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn derives_of(e: &Extracted) -> Vec<&str> {
        e.attrs
            .iter()
            .flat_map(|attr| attr.typepaths.iter().map(String::as_str))
            .collect()
    }

    #[test]
    fn extract_items_parses_struct_with_single_derive() {
        let extracted = extract_items("#[derive(Clone)]\nstruct Foo;\n").unwrap();
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].name, "Foo");
        assert_eq!(extracted[0].line, 2);
        assert_eq!(derives_of(&extracted[0]), ["Clone"]);
        assert_eq!(extracted[0].attrs.len(), 1);
    }

    #[test]
    fn extract_items_parses_multiple_derives_in_one_attr() {
        let extracted = extract_items("#[derive(Clone, Debug)]\nstruct Foo;\n").unwrap();
        assert_eq!(derives_of(&extracted[0]), ["Clone", "Debug"]);
        assert_eq!(extracted[0].attrs.len(), 1);
    }

    #[test]
    fn extract_items_flattens_derives_across_multiple_attrs() {
        let extracted = extract_items("#[derive(Clone)]\n#[derive(Debug)]\nstruct Foo;\n").unwrap();
        assert_eq!(derives_of(&extracted[0]), ["Clone", "Debug"]);
        assert_eq!(extracted[0].attrs.len(), 2);
    }

    #[test]
    fn extract_items_parses_enum() {
        let extracted = extract_items("#[derive(Clone)]\nenum Foo { A, B }\n").unwrap();
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].name, "Foo");
    }

    #[test]
    fn extract_items_skips_non_struct_or_enum_items() {
        let extracted = extract_items("fn hello() {}\n#[derive(Clone)]\nstruct Foo;\n").unwrap();
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].name, "Foo");
    }

    #[test]
    fn extract_items_preserves_qualified_derive_paths() {
        let extracted = extract_items("#[derive(serde::Serialize)]\nstruct Foo;\n").unwrap();
        assert_eq!(derives_of(&extracted[0]), ["serde::Serialize"]);
    }

    #[test]
    fn edit_source_replaces_single_attr_with_placeholder() {
        let mut source = String::from("#[derive(Clone)]\nstruct Foo;\n");
        let extracted = vec![Extracted {
            name: "Foo".into(),
            line: 2,
            attrs: [DeriveAttr {
                typepaths: [String::from("Clone")].into(),
                byte_range: 0..16,
                line_range: 1..1,
            }]
            .into(),
        }];

        let items = edit_source(&mut source, extracted);

        assert_eq!(source, format!("{PLACEHOLDER_MARKER}\n\nstruct Foo;\n"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name(), "Foo");
        assert_eq!(items[0].lineno_source(), 2);
        assert_eq!(items[0].lineno_sandbox(), 3);
    }

    #[test]
    fn edit_source_replaces_multiple_attrs_on_one_item() {
        let mut source = String::from("#[derive(Clone)]\n#[derive(Debug)]\nstruct Foo;\n");
        let extracted = vec![Extracted {
            name: "Foo".into(),
            line: 3,
            attrs: [
                DeriveAttr {
                    typepaths: vec![String::from("Clone")],
                    byte_range: 0..16,
                    line_range: 1..1,
                },
                DeriveAttr {
                    typepaths: vec![String::from("Debug")],
                    byte_range: 17..33,
                    line_range: 2..2,
                },
            ]
            .into(),
        }];

        let items = edit_source(&mut source, extracted);

        assert_eq!(
            source,
            format!("{PLACEHOLDER_MARKER}\n\n{PLACEHOLDER_MARKER}\n\nstruct Foo;\n")
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lineno_source(), 3);
        assert_eq!(items[0].lineno_sandbox(), 5);
    }

    #[test]
    fn edit_source_handles_multiple_items() {
        let mut source = String::from("#[derive(Clone)]\nstruct A;\n#[derive(Debug)]\nstruct B;\n");
        let extracted = vec![
            Extracted {
                name: "A".to_string(),
                line: 2,
                attrs: [DeriveAttr {
                    typepaths: vec!["Clone".to_string()],
                    byte_range: 0..16,
                    line_range: 1..1,
                }]
                .into(),
            },
            Extracted {
                name: "B".to_string(),
                line: 4,
                attrs: [DeriveAttr {
                    typepaths: vec!["Debug".to_string()],
                    byte_range: 27..43,
                    line_range: 3..3,
                }]
                .into(),
            },
        ];

        let items = edit_source(&mut source, extracted);

        assert_eq!(
            source,
            format!("{PLACEHOLDER_MARKER}\n\nstruct A;\n{PLACEHOLDER_MARKER}\n\nstruct B;\n")
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].lineno_sandbox(), 3);
        assert_eq!(items[1].lineno_sandbox(), 6);
    }
}
