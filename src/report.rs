use std::fmt;

use crate::items::{Item, Items};

impl Items {
    /// Sort items within each file by source line number. The path order is
    /// already maintained by the underlying `BTreeMap`.
    pub fn sort(&mut self) {
        for (_, items) in self.iter_mut() {
            items.sort_by_key(Item::lineno_source);
        }
    }

    pub fn total_unused(&self) -> usize {
        self.iter()
            .flat_map(|(_, items)| items.iter())
            .map(|i| i.derives_unused().len())
            .sum()
    }
}

impl fmt::Display for Items {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.total_unused() == 0 {
            return writeln!(f, "No unused derives found.");
        }

        for (path, items) in self.iter() {
            for item in items.iter().filter(|i| !i.derives_unused().is_empty()) {
                writeln!(
                    f,
                    "{}:{} {} — unused: {}",
                    path.display(),
                    item.lineno_source(),
                    item.name(),
                    item.derives_unused().join(", ")
                )?;
            }
        }
        Ok(())
    }
}
