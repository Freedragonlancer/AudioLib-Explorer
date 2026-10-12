// crates/audiolib-db/src/id_membership.rs
//! Exact filter membership for a bounded ranking, with compact dense-ID storage.
use std::collections::HashSet;

const MAX_DENSE_SPAN: usize = 8_388_608;
const MAX_SPAN_PER_RANKED_ID: usize = 8;
// Tiny scattered rankings are cheaper as a HashSet and retain its existing domain.
const MIN_TWO_DENSE_IDS: usize = 128;

pub(crate) enum IdMembership {
    Dense(DenseMembership),
    TwoDense(DenseMembership, DenseMembership),
    Sparse(HashSet<i64>),
}

pub(crate) struct DenseMembership {
    base: i64,
    span: usize,
    words: Vec<u64>,
}

impl DenseMembership {
    fn new(base: i64, span: usize) -> Self {
        Self {
            base,
            span,
            words: vec![0; span.div_ceil(64)],
        }
    }

    fn position(&self, id: i64) -> Option<(usize, u64)> {
        // checked subtraction rejects extreme IDs outside the domain without
        // overflowing; every in-domain offset is at most MAX_DENSE_SPAN - 1.
        let offset = usize::try_from(id.checked_sub(self.base)?).ok()?;
        (offset < self.span).then_some((offset / 64, 1u64 << (offset % 64)))
    }

    fn insert(&mut self, id: i64) -> Option<bool> {
        let (word, mask) = self.position(id)?;
        let was_present = self.words[word] & mask != 0;
        self.words[word] |= mask;
        Some(!was_present)
    }

    fn remove(&mut self, id: i64) -> Option<bool> {
        let (word, mask) = self.position(id)?;
        let was_present = self.words[word] & mask != 0;
        self.words[word] &= !mask;
        Some(was_present)
    }

    fn contains(&self, id: i64) -> Option<bool> {
        self.position(id)
            .map(|(word, mask)| self.words[word] & mask != 0)
    }
}

impl IdMembership {
    /// Use the current HashSet path when discovering a dense domain is unnecessary.
    pub(crate) fn sparse(capacity: usize) -> Self {
        Self::Sparse(HashSet::with_capacity(capacity))
    }

    /// Choose one or two dense domains without reordering the BM25 identities.
    /// Combined bitmap allocations stay within 1 MiB, including word rounding.
    /// All ranked IDs must fit; sparse/extreme domains retain the original HashSet.
    /// Each linear discovery pass observes cancellation at least every 1024 IDs.
    pub(crate) fn for_ranked_ids<E>(
        ids: &[i64],
        mut check_cancel: impl FnMut() -> Result<(), E>,
    ) -> Result<Self, E> {
        check_cancel()?;
        let Some(&first) = ids.first() else {
            return Ok(Self::sparse(0));
        };
        let (mut min, mut max) = (first, first);
        for (index, &id) in ids.iter().enumerate() {
            if index.is_multiple_of(1024) {
                check_cancel()?;
            }
            min = min.min(id);
            max = max.max(id);
        }
        check_cancel()?;
        // The full i64::MIN..=i64::MAX domain has 2^64 elements. Compute before
        // narrowing; distant compact clusters may still fit separate bitsets.
        let span = i128::from(max) - i128::from(min) + 1;
        if span <= MAX_DENSE_SPAN as i128
            && span <= ids.len().saturating_mul(MAX_SPAN_PER_RANKED_ID) as i128
        {
            let span = usize::try_from(span).expect("Bounded positive dense-ID span");
            return Ok(Self::Dense(DenseMembership::new(min, span)));
        }
        if ids.len() < MIN_TWO_DENSE_IDS {
            return Ok(Self::sparse(ids.len()));
        }

        // Legacy small identities and subsequent epoch identities form two dense
        // clusters. Anchor a bounded first window at the minimum, then derive the
        // second window from every remaining ID. Three scattered clusters, a
        // sparse second domain or an excessive total span all fall back exactly.
        let width = MAX_DENSE_SPAN.min(ids.len().saturating_mul(MAX_SPAN_PER_RANKED_ID));
        let mut left_max = min;
        let mut left_count = 0usize;
        let mut right_min = max;
        let mut right_count = 0usize;
        for (index, &id) in ids.iter().enumerate() {
            if index.is_multiple_of(1024) {
                check_cancel()?;
            }
            if i128::from(id) - i128::from(min) < width as i128 {
                left_max = left_max.max(id);
                left_count += 1;
            } else {
                right_min = right_min.min(id);
                right_count += 1;
            }
        }
        check_cancel()?;
        let left_span = i128::from(left_max) - i128::from(min) + 1;
        let right_span = i128::from(max) - i128::from(right_min) + 1;
        if left_count == 0
            || right_count == 0
            || left_span > left_count.saturating_mul(MAX_SPAN_PER_RANKED_ID) as i128
            || right_span > right_count.saturating_mul(MAX_SPAN_PER_RANKED_ID) as i128
            || left_span + right_span > MAX_DENSE_SPAN as i128
        {
            return Ok(Self::sparse(ids.len()));
        }
        let left_span = usize::try_from(left_span).expect("Bounded positive first ID span");
        let right_span = usize::try_from(right_span).expect("Bounded positive second ID span");
        // Two separately rounded vectors can need one extra word even when the
        // exact bit counts fit. Reject before allocating either vector.
        if left_span.div_ceil(64) + right_span.div_ceil(64) > MAX_DENSE_SPAN / 64 {
            return Ok(Self::sparse(ids.len()));
        }
        Ok(Self::TwoDense(
            DenseMembership::new(min, left_span),
            DenseMembership::new(right_min, right_span),
        ))
    }

    /// Insert an SQL candidate. Dense candidates outside the ranking domain cannot
    /// match a ranked ID and are ignored; the caller still counts every SQL row.
    pub(crate) fn insert(&mut self, id: i64) -> bool {
        match self {
            Self::Dense(dense) => dense.insert(id).unwrap_or(false),
            Self::TwoDense(left, right) => left
                .insert(id)
                .or_else(|| right.insert(id))
                .unwrap_or(false),
            Self::Sparse(ids) => ids.insert(id),
        }
    }

    /// Remove a missing record using the same transaction's identity set.
    pub(crate) fn remove(&mut self, id: &i64) -> bool {
        match self {
            Self::Dense(dense) => dense.remove(*id).unwrap_or(false),
            Self::TwoDense(left, right) => left
                .remove(*id)
                .or_else(|| right.remove(*id))
                .unwrap_or(false),
            Self::Sparse(ids) => ids.remove(id),
        }
    }

    /// Test exact membership while walking the unchanged BM25 ranking order.
    pub(crate) fn contains(&self, id: &i64) -> bool {
        match self {
            Self::Dense(dense) => dense.contains(*id).unwrap_or(false),
            Self::TwoDense(left, right) => left
                .contains(*id)
                .or_else(|| right.contains(*id))
                .unwrap_or(false),
            Self::Sparse(ids) => ids.contains(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_membership(ids: &[i64]) -> IdMembership {
        IdMembership::for_ranked_ids(ids, || Ok::<(), std::convert::Infallible>(())).unwrap()
    }

    #[test]
    fn dense_candidates_preserve_exact_membership_and_ignore_outside_domain() {
        let mut membership = make_membership(&[18, 10, 15, 12]);
        assert!(matches!(membership, IdMembership::Dense(_)));
        let mut expected = HashSet::new();
        for id in [10, 15, 18, 15] {
            assert_eq!(membership.insert(id), expected.insert(id));
        }
        for outside in [i64::MIN, 9, 19, i64::MAX] {
            assert!(!membership.insert(outside));
            assert!(!membership.contains(&outside));
            assert!(!membership.remove(&outside));
        }
        assert!(membership.remove(&15));
        expected.remove(&15);
        assert!(!membership.remove(&15));
        for id in 0..30 {
            assert_eq!(membership.contains(&id), expected.contains(&id));
        }
    }

    #[test]
    fn empty_ranking_uses_safe_sparse_membership() {
        let mut membership = make_membership(&[]);
        assert!(matches!(membership, IdMembership::Sparse(_)));
        assert!(!membership.contains(&0));
        assert!(membership.insert(i64::MAX));
        assert!(membership.contains(&i64::MAX));
        assert!(membership.remove(&i64::MAX));
    }

    #[test]
    fn sparse_rankings_keep_hashset_even_when_the_span_fits_the_absolute_limit() {
        let mut membership = make_membership(&[1000, 1]);
        assert!(matches!(membership, IdMembership::Sparse(_)));
        for id in [i64::MIN, 1, 1000, i64::MAX] {
            assert!(membership.insert(id));
            assert!(membership.contains(&id));
            assert!(membership.remove(&id));
            assert!(!membership.contains(&id));
        }
    }

    #[test]
    fn full_signed_identity_range_cannot_overflow_or_select_dense_storage() {
        let mut membership = make_membership(&[i64::MAX, i64::MIN]);
        assert!(matches!(membership, IdMembership::Sparse(_)));
        for id in [i64::MIN, -1, 0, i64::MAX] {
            assert!(membership.insert(id));
            assert!(membership.contains(&id));
        }
        assert!(membership.remove(&i64::MIN));
        assert!(!membership.contains(&i64::MIN));
        assert!(membership.contains(&i64::MAX));
    }

    #[test]
    fn negative_and_extreme_dense_domains_handle_every_boundary_safely() {
        for ids in [
            [-3, -1, 2],
            [i64::MIN, i64::MIN + 1, i64::MIN + 8],
            [i64::MAX - 8, i64::MAX - 1, i64::MAX],
        ] {
            let mut membership = make_membership(&ids);
            assert!(matches!(membership, IdMembership::Dense(_)));
            for id in ids {
                assert!(membership.insert(id));
                assert!(membership.contains(&id));
            }
            for outside in [ids[0].checked_sub(1), ids[2].checked_add(1)]
                .into_iter()
                .flatten()
            {
                assert!(!membership.insert(outside));
                assert!(!membership.contains(&outside));
            }
            for id in ids {
                assert!(membership.remove(&id));
                assert!(!membership.contains(&id));
            }
        }
    }

    #[test]
    fn dense_storage_stays_within_one_mib_at_its_absolute_span_boundary() {
        // Eight-wide gaps meet the density limit exactly. Keeping the original
        // maximum when appending another ID also exceeds the two-window budget.
        let mut ids = (0..MAX_DENSE_SPAN / MAX_SPAN_PER_RANKED_ID)
            .map(|index| (index * MAX_SPAN_PER_RANKED_ID) as i64)
            .collect::<Vec<_>>();
        *ids.last_mut().unwrap() = i64::try_from(MAX_DENSE_SPAN - 1).unwrap();
        let membership = make_membership(&ids);
        let IdMembership::Dense(dense) = membership else {
            panic!("The maximum allowed dense span must use a bitset");
        };
        assert_eq!(dense.words.len() * std::mem::size_of::<u64>(), 1_048_576);
        ids.push(MAX_DENSE_SPAN as i64); // Still within the density budget.
        assert!(matches!(make_membership(&ids), IdMembership::Sparse(_)));
    }

    #[test]
    fn two_dense_domains_preserve_exact_membership_and_ignore_outside_candidates() {
        const EPOCH: i64 = 1_834_000_000_000_000;
        // Discovery sees score order rather than sorted identities.
        let ids = (EPOCH..EPOCH + 128)
            .rev()
            .chain(-64..64)
            .collect::<Vec<_>>();
        let original = ids.clone();
        let mut membership = make_membership(&ids);
        assert!(matches!(membership, IdMembership::TwoDense(_, _)));
        assert_eq!(ids, original);
        let mut expected = HashSet::new();
        for &id in &ids {
            if id % 3 == 0 {
                assert_eq!(membership.insert(id), expected.insert(id));
                assert!(!membership.insert(id));
            }
        }
        for outside in [i64::MIN, -65, 64, EPOCH - 1, EPOCH + 128, i64::MAX] {
            assert!(!membership.insert(outside));
            assert!(!membership.contains(&outside));
            assert!(!membership.remove(&outside));
        }
        for &id in &ids {
            if id % 5 == 0 {
                assert_eq!(membership.remove(&id), expected.remove(&id));
            }
            assert_eq!(membership.contains(&id), expected.contains(&id));
        }
    }

    #[test]
    fn one_epoch_outlier_and_continued_growth_use_bounded_two_dense_domains() {
        const EPOCH: i64 = 1_834_000_000_000_000;
        for new_records in [1, 129, 4096] {
            let ids = (0..4096)
                .chain(EPOCH..EPOCH + new_records)
                .collect::<Vec<_>>();
            let mut membership = make_membership(&ids);
            let IdMembership::TwoDense(left, right) = &membership else {
                panic!("Both old IDs and {new_records} new epoch IDs must use bitsets");
            };
            assert!((left.words.capacity() + right.words.capacity()) * 8 <= 1_048_576);
            for &id in &ids {
                assert!(membership.insert(id));
                assert!(membership.contains(&id));
            }
            assert!(membership.remove(&0));
            assert!(membership.remove(&EPOCH));
            assert!(!membership.contains(&0));
            assert!(!membership.contains(&EPOCH));
            assert!(!membership.contains(&4096));
            assert!(!membership.contains(&(EPOCH - 1)));
        }
    }

    #[test]
    fn two_dense_signed_extreme_clusters_do_not_overflow() {
        let ids = (i64::MIN..i64::MIN + 128)
            .chain(i64::MAX - 127..=i64::MAX)
            .collect::<Vec<_>>();
        let mut membership = make_membership(&ids);
        assert!(matches!(membership, IdMembership::TwoDense(_, _)));
        for &id in &ids {
            assert!(membership.insert(id));
            assert!(membership.contains(&id));
        }
        for outside in [i64::MIN + 128, -1, 0, i64::MAX - 128] {
            assert!(!membership.insert(outside));
            assert!(!membership.contains(&outside));
        }
        assert!(membership.remove(&i64::MIN));
        assert!(membership.remove(&i64::MAX));
        assert!(!membership.contains(&i64::MIN));
        assert!(!membership.contains(&i64::MAX));
    }

    #[test]
    fn sparse_or_three_cluster_rankings_retain_exact_hashset_fallback() {
        const EPOCH: i64 = 1_834_000_000_000_000;
        for ids in [
            (0..128).map(|id| id * 1_000_000).collect::<Vec<_>>(),
            (0..128)
                .chain([1_000_000])
                .chain(EPOCH..EPOCH + 128)
                .collect(),
            (0..128).chain([EPOCH, EPOCH + 1000]).collect(),
        ] {
            let mut membership = make_membership(&ids);
            assert!(matches!(membership, IdMembership::Sparse(_)));
            let mut expected = HashSet::new();
            for id in ids.into_iter().chain([i64::MIN, -1, 0, i64::MAX]) {
                assert_eq!(membership.insert(id), expected.insert(id));
                assert_eq!(membership.contains(&id), expected.contains(&id));
            }
        }
    }

    #[test]
    fn combined_bitmap_budget_includes_each_vectors_word_rounding() {
        const EPOCH: i64 = 1_834_000_000_000_000;
        // Repeats isolate allocation bounds without millions of distinct rows.
        let mut ids = vec![0; MAX_DENSE_SPAN / MAX_SPAN_PER_RANKED_ID];
        *ids.last_mut().unwrap() = (MAX_DENSE_SPAN - 65) as i64;
        ids.extend(EPOCH..EPOCH + 64);
        let membership = make_membership(&ids);
        let IdMembership::TwoDense(left, right) = membership else {
            panic!("Two fully rounded domains must fit exactly at the bitmap budget");
        };
        assert_eq!(
            (left.words.capacity() + right.words.capacity()) * 8,
            1_048_576
        );

        // The exact spans still sum to MAX_DENSE_SPAN, but separately rounding
        // (MAX_DENSE_SPAN-63) and 63 needs one extra word. Never allocate it.
        ids[MAX_DENSE_SPAN / MAX_SPAN_PER_RANKED_ID - 1] += 1;
        ids.pop();
        assert!(matches!(make_membership(&ids), IdMembership::Sparse(_)));
    }

    #[test]
    fn cancellation_is_observed_during_each_discovery_pass() {
        use std::cell::Cell;
        const EPOCH: i64 = 1_834_000_000_000_000;
        let ids = (0..4096).chain(EPOCH..EPOCH + 1024).collect::<Vec<_>>();
        let first_pass_checks = 1 + ids.len().div_ceil(1024) + 1;
        for stop_after in [3, first_pass_checks + 2] {
            let calls = Cell::new(0);
            let result = IdMembership::for_ranked_ids(&ids, || {
                calls.set(calls.get() + 1);
                if calls.get() == stop_after {
                    Err("cancelled")
                } else {
                    Ok(())
                }
            });
            assert!(matches!(result, Err("cancelled")));
            assert_eq!(calls.get(), stop_after);
        }
        assert!(matches!(
            IdMembership::for_ranked_ids(&[], || Err::<(), _>("cancelled")),
            Err("cancelled")
        ));
    }
}
