//! What a library sync changed, worked out from the cache before it and the
//! items it fetched. Nothing here reads or writes the store.

use std::collections::{HashMap, HashSet};

use super::model::MediaId;

/// What [`reconcile`] needs to know of one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known<'a> {
    pub id: &'a MediaId,
    /// The file path the source reported, if any.
    pub path: Option<&'a str>,
    pub runtime_ms: u64,
}

/// How the library changed between two syncs. Items added by the new sync
/// are not listed: nothing is scheduled with them yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryChanges {
    /// Items whose id changed but whose file path stayed the same, as
    /// `(old id, new id)`. They are the same item.
    pub renamed: Vec<(MediaId, MediaId)>,
    /// Items whose runtime changed, by their new id.
    pub resized: Vec<MediaId>,
    /// Items the new sync has neither by id nor by path, by their old id.
    pub vanished: Vec<MediaId>,
}

impl LibraryChanges {
    /// Whether an event that plays `id` needs a new schedule: the item's
    /// runtime changed or it is gone. A renamed item does not count; its
    /// events take the new id.
    pub fn affects(&self, id: &MediaId) -> bool {
        self.resized.contains(id) || self.vanished.contains(id)
    }

    pub fn is_empty(&self) -> bool {
        self.renamed.is_empty() && self.resized.is_empty() && self.vanished.is_empty()
    }
}

/// Matches each item of `old`, the cache before a sync, with one of `new`,
/// the items the sync fetched: by id, or else by file path with an item whose
/// id `old` does not have. Items are listed in `old`'s order.
pub fn reconcile(old: &[Known<'_>], new: &[Known<'_>]) -> LibraryChanges {
    let old_ids: HashSet<&MediaId> = old.iter().map(|k| k.id).collect();
    let by_id: HashMap<&MediaId, &Known<'_>> = new.iter().map(|k| (k.id, k)).collect();
    let mut by_path: HashMap<&str, &Known<'_>> = HashMap::new();
    for known in new.iter().filter(|k| !old_ids.contains(k.id)) {
        if let Some(path) = known.path {
            by_path.entry(path).or_insert(known);
        }
    }

    let mut changes = LibraryChanges::default();
    for before in old {
        let matched = by_id.get(before.id).copied().or_else(|| {
            let after = by_path.remove(before.path?)?;
            changes.renamed.push((before.id.clone(), after.id.clone()));
            Some(after)
        });
        match matched {
            Some(after) if after.runtime_ms != before.runtime_ms => {
                changes.resized.push(after.id.clone())
            }
            Some(_) => {}
            None => changes.vanished.push(before.id.clone()),
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<MediaId> {
        names.iter().map(|n| MediaId::new(*n)).collect()
    }

    fn known<'a>(id: &'a MediaId, path: Option<&'a str>, runtime_ms: u64) -> Known<'a> {
        Known {
            id,
            path,
            runtime_ms,
        }
    }

    #[test]
    fn an_unchanged_library_has_no_changes() {
        let id = ids(&["a", "b"]);
        let items = [known(&id[0], Some("/a"), 1), known(&id[1], None, 2)];
        assert!(reconcile(&items, &items).is_empty());
    }

    #[test]
    fn a_new_runtime_under_the_same_id_is_a_resize() {
        let id = ids(&["a", "b"]);
        let old = [known(&id[0], Some("/a"), 1), known(&id[1], Some("/b"), 2)];
        let new = [known(&id[0], Some("/a"), 1), known(&id[1], Some("/b"), 3)];
        let changes = reconcile(&old, &new);
        assert_eq!(changes.resized, ids(&["b"]));
        assert!(changes.renamed.is_empty() && changes.vanished.is_empty());
    }

    #[test]
    fn a_new_id_with_the_same_path_is_the_same_item() {
        let id = ids(&["a", "a2"]);
        let old = [known(&id[0], Some("/a.mkv"), 1)];
        let new = [known(&id[1], Some("/a.mkv"), 5)];
        let changes = reconcile(&old, &new);
        assert_eq!(changes.renamed, [(id[0].clone(), id[1].clone())]);
        assert_eq!(changes.resized, ids(&["a2"]));
        assert!(changes.vanished.is_empty());
    }

    #[test]
    fn an_item_missing_by_id_and_path_vanished() {
        let id = ids(&["a", "b", "c", "d"]);
        // `b` had no path; `c`'s path now belongs to `a`, which was cached
        // before, so `c` cannot be matched with it.
        let old = [
            known(&id[0], Some("/a"), 1),
            known(&id[1], None, 1),
            known(&id[2], Some("/c"), 1),
        ];
        let new = [known(&id[0], Some("/c"), 1), known(&id[3], None, 1)];
        let changes = reconcile(&old, &new);
        assert_eq!(changes.vanished, ids(&["b", "c"]));
        assert!(changes.renamed.is_empty() && changes.resized.is_empty());
    }

    #[test]
    fn one_new_item_matches_only_one_old_path() {
        let id = ids(&["a", "b", "n"]);
        let old = [known(&id[0], Some("/x"), 1), known(&id[1], Some("/x"), 1)];
        let new = [known(&id[2], Some("/x"), 1)];
        let changes = reconcile(&old, &new);
        assert_eq!(changes.renamed, [(id[0].clone(), id[2].clone())]);
        assert_eq!(changes.vanished, ids(&["b"]));
    }

    #[test]
    fn only_changes_to_scheduled_runtimes_or_vanished_items_affect_events() {
        let changes = LibraryChanges {
            renamed: vec![(MediaId::new("a"), MediaId::new("a2"))],
            resized: ids(&["b"]),
            vanished: ids(&["c"]),
        };
        assert!(!changes.affects(&MediaId::new("a")));
        assert!(!changes.affects(&MediaId::new("a2")));
        assert!(changes.affects(&MediaId::new("b")));
        assert!(changes.affects(&MediaId::new("c")));
    }
}
