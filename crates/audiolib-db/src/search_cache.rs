// crates/audiolib-db/src/search_cache.rs
//! Bounded FTS ranking identities; Metadata is always hydrated from the current snapshot.
use std::{collections::VecDeque, sync::Arc};

pub(crate) const MAX_RANKED_IDS: usize = 2_097_152;
const MAX_CACHED_IDS: usize = 4_194_304;
const MAX_ENTRIES: usize = 16;

struct Entry {
    query: String,
    ids: Arc<Vec<i64>>,
}

#[derive(Default)]
pub(crate) struct RankingCache {
    revision: Option<u64>,
    entries: VecDeque<Entry>,
    ids: usize,
}

impl RankingCache {
    pub(crate) fn get(&mut self, revision: u64, query: &str) -> Option<Arc<Vec<i64>>> {
        if self.revision != Some(revision) {
            self.entries.clear();
            self.ids = 0;
            self.revision = Some(revision);
        }
        let index = self.entries.iter().position(|entry| entry.query == query)?;
        let entry = self.entries.remove(index)?;
        let ids = entry.ids.clone();
        self.entries.push_back(entry);
        Some(ids)
    }

    pub(crate) fn insert(&mut self, revision: u64, query: String, ids: Arc<Vec<i64>>) {
        self.get(revision, &query);
        if ids.len() > MAX_RANKED_IDS {
            return;
        }
        if let Some(index) = self.entries.iter().position(|entry| entry.query == query)
            && let Some(previous) = self.entries.remove(index)
        {
            self.ids -= previous.ids.len();
        }
        while self.ids + ids.len() > MAX_CACHED_IDS || self.entries.len() >= MAX_ENTRIES {
            if let Some(previous) = self.entries.pop_front() {
                self.ids -= previous.ids.len();
            }
        }
        self.ids += ids.len();
        self.entries.push_back(Entry { query, ids });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_rankings_are_bounded_and_every_commit_invalidates_them() {
        let mut cache = RankingCache::default();
        for index in 0..20 {
            cache.insert(
                1,
                index.to_string(),
                Arc::new(vec![index; MAX_RANKED_IDS / 4]),
            );
            assert!(cache.ids <= MAX_CACHED_IDS);
            assert!(cache.entries.len() <= MAX_ENTRIES);
        }
        assert!(cache.get(1, "0").is_none());
        assert!(cache.get(1, "19").is_some());
        assert!(cache.get(2, "19").is_none());
        assert_eq!(cache.ids, 0);
    }
}
