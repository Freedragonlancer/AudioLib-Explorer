// app/audiolib-desktop/src/artwork_album_hints.rs
//! Bounded shared-entity cache hints, independent of the selected member WAV.
use super::*;

const MAX_ALBUM_HINTS: usize = 128;
#[derive(Clone, PartialEq, Eq)]
struct AlbumScope {
    source_generation: u64,
    namespace: String,
    remote: Option<(String, u64)>,
}
#[derive(Clone, PartialEq, Eq)]
struct AlbumHint {
    scope: AlbumScope,
    album_id: i64,
    cache_path: PathBuf,
}
#[derive(Default)]
pub(super) struct ArtworkAlbumHints {
    hints: VecDeque<AlbumHint>,
}
impl ArtworkAlbumHints {
    fn remember(&mut self, hint: AlbumHint) {
        self.hints
            .retain(|old| old.scope != hint.scope || old.album_id != hint.album_id);
        self.hints.push_back(hint);
        while self.hints.len() > MAX_ALBUM_HINTS {
            self.hints.pop_front();
        }
    }
    pub(super) fn clear(&mut self) {
        self.hints.clear();
    }
}
impl Controller {
    fn artwork_album_scope(&self, provider: &SqliteProvider) -> Option<AlbumScope> {
        let remote = if self.remote_read_only() {
            let remote = self.remote.lock().unwrap();
            let state = remote.state.as_ref()?;
            Some((state.library_id.clone(), state.revision))
        } else {
            None
        };
        Some(AlbumScope {
            source_generation: self.source_generation.load(Ordering::Acquire),
            namespace: provider.album_namespace(),
            remote,
        })
    }
    pub(super) fn remember_album_artwork(&self, job: &ArtworkJob) {
        if !self.artwork_current(job) {
            return;
        }
        let album = &job.album;
        let provider = self.provider();
        let Some(scope) = self.artwork_album_scope(&provider) else {
            return;
        };
        let Some(cache_path) = self.album_cached_artwork_path(album) else {
            return;
        };
        if scope.source_generation == job.source_generation && self.artwork_current(job) {
            self.artwork_album_hints
                .lock()
                .unwrap()
                .remember(AlbumHint {
                    scope,
                    album_id: album.id,
                    cache_path,
                });
        }
    }
    /// Only cached shared pixels are resolved; source WAVs are never touched here.
    pub(super) fn hinted_album_artwork(
        &self,
        provider: &SqliteProvider,
        album: &Album,
    ) -> Option<PathBuf> {
        let scope = self.artwork_album_scope(provider)?;
        let expected = self.album_cached_artwork_path(album)?;
        self.artwork_album_hints
            .lock()
            .unwrap()
            .hints
            .iter()
            .rev()
            .find(|hint| {
                hint.scope == scope && hint.album_id == album.id && hint.cache_path == expected
            })
            .map(|hint| hint.cache_path.clone())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn representative_hints_evict_old_albums_and_replace_existing_entities() {
        let mut hints = ArtworkAlbumHints::default();
        let scope = AlbumScope {
            source_generation: 1,
            namespace: "library-one".into(),
            remote: None,
        };
        for index in 0..=MAX_ALBUM_HINTS {
            hints.remember(AlbumHint {
                scope: scope.clone(),
                album_id: index as i64,
                cache_path: format!("{index}.png").into(),
            });
        }
        assert_eq!(hints.hints.len(), MAX_ALBUM_HINTS);
        assert!(!hints.hints.iter().any(|hint| hint.album_id == 0));
        let mut replacement = hints.hints.front().unwrap().clone();
        replacement.cache_path = "new-pixels.png".into();
        let id = replacement.album_id;
        hints.remember(replacement);
        assert_eq!(hints.hints.len(), MAX_ALBUM_HINTS);
        assert_eq!(
            hints.hints.back().unwrap().cache_path,
            PathBuf::from("new-pixels.png")
        );
        assert_eq!(
            hints
                .hints
                .iter()
                .filter(|hint| hint.album_id == id)
                .count(),
            1
        );
    }
}
