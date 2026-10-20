// crates/audiolib-db/src/search_cache.rs
//! Bounded FTS ranking identities; Metadata is always hydrated from the current snapshot.
use std::{collections::VecDeque, sync::Arc};

pub(crate) const MAX_RANKED_IDS: usize = 2_097_152;
const MAX_CACHED_IDS: usize = 4_194_304;
const MAX_ENTRIES: usize = 16;

struct Entry {
    query: String,
    ids: Arc<Vec<i64>>,
    fingerprint: u64,
}

// A deterministic, order-sensitive prefilter only. Full sequence equality below
// remains authoritative, including when two different rankings hash alike.
fn ranking_fingerprint(ids: &[i64]) -> u64 {
    ids.iter().fold(0xcbf2_9ce4_8422_2325, |hash, id| {
        (hash ^ (*id as u64)).wrapping_mul(0x0000_0100_0000_01b3)
    })
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
        let fingerprint = ranking_fingerprint(&ids);
        let canonical = self
            .entries
            .iter()
            .find(|entry| {
                entry.ids.len() == ids.len()
                    && entry.fingerprint == fingerprint
                    && (Arc::ptr_eq(&entry.ids, &ids) || entry.ids.as_slice() == ids.as_slice())
            })
            .map(|entry| entry.ids.clone());
        let ids = canonical.unwrap_or(ids);
        let replacement = self.entries.iter().position(|entry| entry.query == query);
        let replaced_payload = replacement.map_or(0, |index| {
            let previous = &self.entries[index];
            if self
                .entries
                .iter()
                .enumerate()
                .any(|(other, entry)| other != index && Arc::ptr_eq(&entry.ids, &previous.ids))
            {
                0
            } else {
                previous.ids.len()
            }
        });
        let added_payload = if self
            .entries
            .iter()
            .enumerate()
            .any(|(index, entry)| Some(index) != replacement && Arc::ptr_eq(&entry.ids, &ids))
        {
            0
        } else {
            ids.len()
        };
        // A broad new query must not flush a working set of smaller rankings.
        // Admission can be refused without changing the complete in-flight
        // result or the per-query/global memory limits. Count only unique payloads
        // that remain after replacement; small queries keep LRU admission.
        if ids.len() > MAX_CACHED_IDS / 8
            && self.ids - replaced_payload + added_payload > MAX_CACHED_IDS
        {
            return;
        }
        if let Some(index) = replacement
            && let Some(previous) = self.entries.remove(index)
        {
            self.release_payload(&previous.ids);
        }
        loop {
            // Eviction may remove the last cached reference to this candidate's
            // canonical Arc. Recompute its added payload after every removal.
            let added_payload = if self.retains_payload(&ids) {
                0
            } else {
                ids.len()
            };
            if self.ids + added_payload <= MAX_CACHED_IDS && self.entries.len() < MAX_ENTRIES {
                self.ids += added_payload;
                self.entries.push_back(Entry {
                    query,
                    ids,
                    fingerprint,
                });
                return;
            }
            let Some(previous) = self.entries.pop_front() else {
                return;
            };
            self.release_payload(&previous.ids);
        }
    }

    fn retains_payload(&self, ids: &Arc<Vec<i64>>) -> bool {
        self.entries
            .iter()
            .any(|entry| Arc::ptr_eq(&entry.ids, ids))
    }

    fn release_payload(&mut self, ids: &Arc<Vec<i64>>) {
        // In-flight readers may still hold this Arc. Only references retained by
        // cache entries contribute to the cache's 32 MiB identity-payload budget.
        if !self.retains_payload(ids) {
            self.ids -= ids.len();
        }
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

    #[test]
    fn broad_queries_cannot_turn_a_slightly_oversized_working_set_into_all_misses() {
        let mut cache = RankingCache::default();
        let sizes = [
            100_000, 1_000_000, 100_000, 200_000, 1_000_000, 100_000, 0, 100_000, 1_000, 100_000,
            100_000, 100_000, 100_000, 1_000_000, 100_000, 100_000,
        ];
        let mut hits = 0;
        for round in 0..3 {
            hits = 0;
            for (query, size) in sizes.into_iter().enumerate() {
                let query = query.to_string();
                if cache.get(1, &query).is_some() {
                    hits += 1;
                } else {
                    let identity = query.parse::<i64>().unwrap();
                    cache.insert(1, query, Arc::new(vec![identity; size]));
                }
                assert!(cache.ids <= MAX_CACHED_IDS, "round {round}");
                assert!(cache.entries.len() <= MAX_ENTRIES);
            }
        }
        assert!(
            hits >= 15,
            "the retained working set must stabilize: {hits}"
        );
    }

    #[test]
    fn refused_large_admission_preserves_previous_entries_and_revision_reset() {
        let mut cache = RankingCache::default();
        let original = Arc::new(vec![1; 200_000]);
        cache.insert(1, "small".into(), original.clone());
        for (index, query) in ["large-a", "large-b", "large-c"].into_iter().enumerate() {
            cache.insert(
                1,
                query.into(),
                Arc::new(vec![index as i64 + 10; 1_000_000]),
            );
        }
        let before = cache.ids;
        cache.insert(1, "small".into(), Arc::new(vec![2; MAX_RANKED_IDS]));
        assert_eq!(cache.ids, before);
        assert!(Arc::ptr_eq(&cache.get(1, "small").unwrap(), &original));
        cache.insert(1, "new-large".into(), Arc::new(vec![0; MAX_RANKED_IDS]));
        assert!(cache.get(1, "new-large").is_none());
        assert_eq!(cache.ids, before);
        cache.insert(2, "new-large".into(), Arc::new(vec![0; MAX_RANKED_IDS]));
        assert_eq!(cache.ids, MAX_RANKED_IDS);
        assert!(cache.get(2, "new-large").is_some());
        assert!(cache.get(2, "small").is_none());
    }

    #[test]
    fn different_queries_with_identical_ordered_ids_share_one_payload() {
        let mut cache = RankingCache::default();
        let original = Arc::new(vec![8, 3, 5]);
        let duplicate = Arc::new(vec![8, 3, 5]);
        assert!(!Arc::ptr_eq(&original, &duplicate));
        cache.insert(1, "metal".into(), original.clone());
        cache.insert(1, "metal".into(), duplicate.clone());
        assert_eq!(cache.ids, 3, "same-key replacement retains one payload");
        cache.insert(1, "et".into(), duplicate.clone());
        let metal = cache.get(1, "metal").unwrap();
        let fragment = cache.get(1, "et").unwrap();
        assert!(Arc::ptr_eq(&metal, &original));
        assert!(Arc::ptr_eq(&metal, &fragment));
        assert!(!Arc::ptr_eq(&fragment, &duplicate));
        assert_eq!(cache.ids, 3);
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn a_fingerprint_collision_cannot_share_differently_ordered_ids() {
        let mut cache = RankingCache::default();
        let original = Arc::new(vec![8, 3, 5]);
        let different = Arc::new(vec![5, 3, 8]);
        cache.insert(1, "first".into(), original.clone());
        // Force the inexpensive prefilter to collide. Exact sequence equality
        // must still protect both the result order and payload accounting.
        cache.entries.back_mut().unwrap().fingerprint = ranking_fingerprint(&different);
        cache.insert(1, "second".into(), different.clone());
        let first = cache.get(1, "first").unwrap();
        let second = cache.get(1, "second").unwrap();
        assert!(Arc::ptr_eq(&first, &original));
        assert!(Arc::ptr_eq(&second, &different));
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(cache.ids, 6);
    }

    #[test]
    fn replacements_release_only_the_last_cached_payload_reference() {
        let mut cache = RankingCache::default();
        let in_flight = Arc::new(vec![1; 10]);
        cache.insert(1, "first".into(), in_flight.clone());
        cache.insert(1, "second".into(), Arc::new(vec![1; 10]));
        assert_eq!(cache.ids, 10);
        cache.insert(1, "first".into(), Arc::new(vec![2; 5]));
        assert_eq!(cache.ids, 15);
        cache.insert(1, "second".into(), Arc::new(vec![3; 7]));
        assert_eq!(
            cache.ids, 12,
            "an in-flight Arc is outside the cache budget"
        );
        assert_eq!(in_flight.len(), 10);
        cache.insert(1, "first".into(), Arc::new(vec![3; 7]));
        assert_eq!(cache.ids, 7);
        assert!(Arc::ptr_eq(
            &cache.get(1, "first").unwrap(),
            &cache.get(1, "second").unwrap()
        ));
    }

    #[test]
    fn entry_eviction_preserves_shared_payload_until_its_last_reference() {
        let mut cache = RankingCache::default();
        let in_flight = Arc::new(vec![1; MAX_RANKED_IDS]);
        cache.insert(1, "shared-first".into(), in_flight.clone());
        cache.insert(1, "shared-second".into(), Arc::new(vec![1; MAX_RANKED_IDS]));
        cache.insert(1, "other".into(), Arc::new(vec![2; MAX_RANKED_IDS]));
        assert_eq!(cache.ids, MAX_CACHED_IDS);
        for index in 0..14 {
            cache.insert(1, format!("empty-{index}"), Arc::new(Vec::new()));
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert_eq!(cache.ids, MAX_CACHED_IDS);
        assert!(
            cache
                .entries
                .iter()
                .all(|entry| entry.query != "shared-first")
        );
        cache.insert(1, "empty-last".into(), Arc::new(Vec::new()));
        assert_eq!(cache.ids, MAX_RANKED_IDS);
        assert!(
            cache
                .entries
                .iter()
                .all(|entry| entry.query != "shared-second")
        );
        assert_eq!(in_flight.len(), MAX_RANKED_IDS);
    }

    #[test]
    fn evicting_the_candidates_canonical_entry_recharges_its_payload() {
        let mut cache = RankingCache::default();
        let canonical = Arc::new(vec![1; MAX_RANKED_IDS]);
        cache.insert(1, "canonical".into(), canonical.clone());
        cache.insert(1, "other".into(), Arc::new(vec![2; MAX_RANKED_IDS]));
        for index in 0..14 {
            cache.insert(1, format!("empty-{index}"), Arc::new(Vec::new()));
        }
        cache.insert(1, "equivalent".into(), Arc::new(vec![1; MAX_RANKED_IDS]));
        assert!(cache.get(1, "canonical").is_none());
        assert!(Arc::ptr_eq(
            &cache.get(1, "equivalent").unwrap(),
            &canonical
        ));
        assert_eq!(cache.ids, MAX_CACHED_IDS);
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn refused_replacement_cannot_discount_a_payload_shared_by_another_key() {
        let mut cache = RankingCache::default();
        let original = Arc::new(vec![1; 200_000]);
        cache.insert(1, "original".into(), original.clone());
        cache.insert(1, "alias".into(), Arc::new(vec![1; 200_000]));
        for (index, query) in ["large-a", "large-b", "large-c"].into_iter().enumerate() {
            cache.insert(
                1,
                query.into(),
                Arc::new(vec![index as i64 + 10; 1_000_000]),
            );
        }
        let before = cache.ids;
        cache.insert(1, "original".into(), Arc::new(vec![2; MAX_RANKED_IDS]));
        assert_eq!(cache.ids, before);
        assert!(Arc::ptr_eq(&cache.get(1, "original").unwrap(), &original));
        assert!(Arc::ptr_eq(&cache.get(1, "alias").unwrap(), &original));
        assert!(cache.get(2, "original").is_none());
        assert_eq!(cache.ids, 0);
        assert!(cache.entries.is_empty());
        cache.insert(2, "too-large".into(), Arc::new(vec![0; MAX_RANKED_IDS + 1]));
        assert_eq!(cache.ids, 0);
        assert!(cache.entries.is_empty());
    }
}
