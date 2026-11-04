// app/audiolib-desktop/src/result_page_navigation.rs
//! Published row ranges own their opaque continuation and backwards positions.
use audiolib_core::{SearchPage, SearchRequest, SearchScope};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Position {
    offset: usize,
    limit: usize,
    cursor: Option<String>,
}

impl Position {
    fn of(request: &SearchRequest) -> Self {
        Self {
            offset: request.offset,
            limit: request.limit,
            cursor: request.cursor.clone(),
        }
    }
    fn apply(&self, request: &mut SearchRequest) {
        request.offset = self.offset;
        request.limit = self.limit;
        request.cursor = self.cursor.clone();
        request.page = None;
    }
}

fn key(request: &SearchRequest) -> Vec<u8> {
    let mut identity = request.clone();
    identity.offset = 0;
    identity.limit = 0;
    identity.cursor = None;
    identity.page = None;
    serde_json::to_vec(&identity).expect("owned search request can be serialized")
}

#[derive(Clone)]
pub(super) struct SourcePage {
    returned: usize,
    total: u64,
    exact: bool,
    next_cursor: Option<String>,
}

impl From<&SearchPage> for SourcePage {
    fn from(page: &SearchPage) -> Self {
        Self {
            returned: page.returned_count,
            total: page.total_count,
            exact: page.total_count_exact,
            next_cursor: page.next_cursor.clone(),
        }
    }
}

impl SourcePage {
    fn has_next(&self, request: &SearchRequest) -> bool {
        if self.next_cursor.is_some() {
            return true;
        }
        if !self.exact
            || (!request.query.trim().is_empty()
                && matches!(request.scope, SearchScope::All | SearchScope::Path))
        {
            return false;
        }
        (request.offset.saturating_add(self.returned) as u64) < self.total
    }
}

struct Transition {
    from: Position,
    forward: bool,
    popped: Option<Position>,
}

#[derive(Default)]
pub(super) struct ResultPageNavigation {
    key: Vec<u8>,
    current: Option<Position>,
    next_cursor: Option<String>,
    returned: usize,
    previous: Vec<Position>,
    requested: bool,
    pending: Option<Transition>,
}

impl ResultPageNavigation {
    /// Do not cache displayed rows under an unfulfilled next/previous request.
    pub(super) fn matches_current_request(&self, request: &SearchRequest) -> bool {
        self.pending.is_none()
            && self.current.as_ref().is_none_or(|position| {
                self.key == key(request) && *position == Position::of(request)
            })
    }

    /// Refreshes and restored queries start with their offset, never a stale cursor.
    pub(super) fn prepare_query(&mut self, request: &mut SearchRequest, reset: bool) {
        if reset || !std::mem::take(&mut self.requested) {
            self.clear();
            request.cursor = None;
        }
    }

    /// Range-prefetch changes its offset/limit and cannot reuse a page cursor.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Request one next/previous page; keep a rollback position until publication.
    pub(super) fn step(&mut self, request: &mut SearchRequest, direction: i32, page_size: usize) {
        let from = Position::of(request);
        if self.key != key(request) || self.current.as_ref() != Some(&from) {
            self.clear();
            self.key = key(request);
            self.current = Some(from.clone());
            self.returned = request.limit;
        }
        let popped = if direction < 0 {
            self.previous.pop()
        } else {
            None
        };
        let target = if direction < 0 {
            popped.clone().unwrap_or(Position {
                offset: from.offset.saturating_sub(page_size),
                limit: page_size,
                cursor: None,
            })
        } else {
            self.previous.push(from.clone());
            Position {
                offset: from.offset.saturating_add(self.returned),
                limit: page_size,
                cursor: if from.limit == page_size {
                    self.next_cursor.clone()
                } else {
                    None
                },
            }
        };
        target.apply(request);
        self.pending = Some(Transition {
            from,
            forward: direction > 0,
            popped,
        });
        self.requested = true;
    }

    /// Navigation is based on the original provider range before removal overlays.
    pub(super) fn published(&mut self, request: &SearchRequest, page: &SourcePage) -> bool {
        let identity = key(request);
        if self.key != identity {
            self.previous.clear();
        }
        self.key = identity;
        self.current = Some(Position::of(request));
        self.returned = page.returned;
        self.next_cursor = page.next_cursor.clone();
        self.requested = false;
        self.pending = None;
        page.has_next(request)
    }

    /// A rejected/stale continuation must not move the still displayed range.
    pub(super) fn failed(&mut self, request: &mut SearchRequest) {
        if let Some(transition) = self.pending.take() {
            if transition.forward {
                self.previous.pop();
            } else if let Some(previous) = transition.popped {
                self.previous.push(previous);
            }
            transition.from.apply(request);
        }
        self.requested = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(returned: usize, total: u64, exact: bool, cursor: Option<&str>) -> SourcePage {
        SourcePage::from(&SearchPage {
            returned_count: returned,
            total_count: total,
            total_count_exact: exact,
            next_cursor: cursor.map(str::to_owned),
            ..Default::default()
        })
    }

    #[test]
    fn provider_continuation_outweighs_totals_and_failed_navigation_restores_the_range() {
        let mut navigation = ResultPageNavigation::default();
        let mut request = SearchRequest {
            query: "wind".into(),
            scope: SearchScope::Path,
            limit: 200,
            ..Default::default()
        };
        // An overlay may hide every row of a page. Its raw provider range and
        // continuation still advance by 200, rather than skipping or stopping.
        assert!(navigation.published(&request, &reply(200, 0, false, Some("next"))));
        navigation.step(&mut request, 1, 200);
        assert_eq!(
            (request.offset, request.cursor.as_deref()),
            (200, Some("next"))
        );
        assert!(!navigation.matches_current_request(&request));
        navigation.prepare_query(&mut request, false);
        navigation.failed(&mut request);
        assert_eq!((request.offset, request.cursor.as_deref()), (0, None));
        assert!(navigation.matches_current_request(&request));
        navigation.step(&mut request, 1, 200);
        navigation.prepare_query(&mut request, false);
        assert!(!navigation.published(&request, &reply(51, 251, true, None)));
        navigation.step(&mut request, -1, 200);
        assert_eq!((request.offset, request.cursor.as_deref()), (0, None));
    }

    #[test]
    fn refresh_and_range_size_changes_do_not_reuse_an_unrelated_cursor() {
        let mut navigation = ResultPageNavigation::default();
        let mut request = SearchRequest {
            query: "wind".into(),
            scope: SearchScope::Path,
            limit: 600,
            ..Default::default()
        };
        assert!(navigation.published(&request, &reply(600, 601, false, Some("wide"))));
        navigation.step(&mut request, 1, 200);
        assert_eq!(
            (request.offset, request.limit, request.cursor.as_deref()),
            (600, 200, None)
        );
        navigation.prepare_query(&mut request, false);
        navigation.published(&request, &reply(200, 801, false, Some("narrow")));
        navigation.step(&mut request, 1, 200);
        assert_eq!(request.cursor.as_deref(), Some("narrow"));
        navigation.prepare_query(&mut request, false);
        navigation.prepare_query(&mut request, false);
        assert_eq!(request.offset, 800);
        assert!(request.cursor.is_none());
    }
}
