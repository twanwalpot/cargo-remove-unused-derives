use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Items {
    items: BTreeMap<PathBuf, Vec<Item>>,
}

/// An item that had its derives stripped.
pub struct Item {
    name: String,
    /// Line number in the user's original source file; used for the end-of-run report.
    lineno_source: usize,
    /// Line number in the sandbox file; used to re-add derive attributes; may shift.
    lineno_sandbox: usize,
    /// Every derive that was on the item in source order. Immutable; restore
    /// reads this to render the slot back in the user's original order.
    derives_original: Box<[String]>,
    /// Derives that haven't been restored yet. Shrinks over the run as the
    /// compiler proves each derive is needed; whatever remains at the end is
    /// the report's "unused" list.
    derives_unused: Vec<String>,
}

impl Items {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn insert(&mut self, path: PathBuf, items: Vec<Item>) {
        self.items.insert(path, items);
    }

    pub fn get(&self, path: &Path) -> Option<&[Item]> {
        self.items.get(path).map(Vec::as_slice)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Path, &[Item])> {
        self.items.iter().map(|(k, v)| (k.as_path(), v.as_slice()))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&Path, &mut [Item])> {
        self.items
            .iter_mut()
            .map(|(k, v)| (k.as_path(), v.as_mut_slice()))
    }

    /// Mutable lookup of one item by `(path, idx)`. Returns the path borrowed
    /// from the map alongside the item, so callers can thread them together
    /// with a single lifetime.
    pub fn get_mut_at(&mut self, path: &Path, idx: usize) -> Option<(&Path, &mut Item)> {
        self.items
            .iter_mut()
            .find(|(k, _)| k.as_path() == path)
            .and_then(|(k, v)| v.get_mut(idx).map(|item| (k.as_path(), item)))
    }
}

impl Item {
    pub fn new(
        name: String,
        lineno_source: usize,
        lineno_sandbox: usize,
        derives: Vec<String>,
    ) -> Self {
        Self {
            name,
            lineno_source,
            lineno_sandbox,
            derives_original: derives.clone().into_boxed_slice(),
            derives_unused: derives,
        }
    }

    /// Get the name of the item e.g `Foo` in `struct Foo`.
    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    /// Line in the user's original source file.
    pub fn lineno_source(&self) -> usize {
        self.lineno_source
    }

    /// Line in the sandbox file.
    pub fn lineno_sandbox(&self) -> usize {
        self.lineno_sandbox
    }

    /// Derives that haven't been restored yet, in source order.
    pub fn derives_unused(&self) -> &[String] {
        &self.derives_unused
    }

    /// Derives that have been restored so far, in source order.
    pub fn derives_restored(&self) -> Vec<&str> {
        self.derives_original
            .iter()
            .filter(|d| !self.derives_unused.iter().any(|u| u == *d))
            .map(String::as_str)
            .collect()
    }

    pub fn mark_restored(&mut self, derive: &str) {
        self.derives_unused.retain(|d| d != derive);
    }
}
