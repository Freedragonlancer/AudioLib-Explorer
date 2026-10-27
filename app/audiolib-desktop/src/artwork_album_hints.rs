// app/audiolib-desktop/src/artwork_album_hints.rs
//! Bounded generated-cover representatives; no pixels or complete Metadata are retained.
use super::*;

const MAX_ALBUM_HINTS: usize = 128;

#[derive(Clone, PartialEq, Eq)]
struct AlbumScope {
    source_generation: u64,
    database: PathBuf,
    remote: Option<(String, u64)>,
}

#[derive(Clone, PartialEq, Eq)]
struct AlbumHint {
    scope: AlbumScope,
    album: String,
    id: i64,
    source_path: String,
    cache_path: PathBuf,
}

#[derive(Default)]
pub(super) struct ArtworkAlbumHints {
    hints: VecDeque<AlbumHint>,
}

impl ArtworkAlbumHints {
    fn remember(&mut self, hint: AlbumHint) {
        self.hints
            .retain(|old| old.scope != hint.scope || old.album != hint.album);
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
        let source_generation = self.source_generation.load(Ordering::Acquire);
        let remote = if self.remote_read_only() {
            let remote = self.remote.lock().unwrap();
            let state = remote.state.as_ref()?;
            Some((state.library_id.clone(), state.revision))
        } else {
            None
        };
        Some(AlbumScope {
            source_generation,
            database: provider.database_path().to_path_buf(),
            remote,
        })
    }

    /// Remember a successfully published member without disk or database work on
    /// the event loop. Capture context before locking the small hint collection.
    pub(super) fn remember_album_artwork(&self, job: &ArtworkJob) {
        let album = audiolib_core::album(&job.file.attributes);
        if album.is_empty() || !self.artwork_current(job) {
            return;
        }
        let provider = self.provider();
        let Some(scope) = self.artwork_album_scope(&provider) else {
            return;
        };
        let Some(cache_path) = self.cached_artwork_path(&job.file) else {
            return;
        };
        if scope.source_generation != job.source_generation
            || !self.artwork_current(job)
            || self.artwork_album_scope(&provider).as_ref() != Some(&scope)
        {
            return;
        }
        let hint = AlbumHint {
            scope,
            album,
            id: job.file.id,
            source_path: job.file.file_path.clone(),
            cache_path,
        };
        self.artwork_album_hints.lock().unwrap().remember(hint);
    }

    /// Resolve one remembered member from the owned database DTO only. The caller
    /// reads cached PNG pixels and falls back if that file has been pruned.
    pub(super) fn hinted_album_artwork(
        &self,
        provider: &SqliteProvider,
        album: &str,
    ) -> Option<PathBuf> {
        let scope = self.artwork_album_scope(provider)?;
        let hint = {
            let hints = self.artwork_album_hints.lock().unwrap();
            hints
                .hints
                .iter()
                .rev()
                .find(|hint| hint.scope == scope && hint.album == album)
                .cloned()
        }?;
        // Never keep the hints mutex while reading a provider or runtime context.
        let valid = provider.get_audio(hint.id).ok().is_some_and(|file| {
            !file.missing
                && file.file_path == hint.source_path
                && audiolib_core::album(&file.attributes) == album
                && self.cached_artwork_path(&file).as_ref() == Some(&hint.cache_path)
        }) && self.artwork_album_scope(provider).as_ref() == Some(&scope);
        if valid {
            return Some(hint.cache_path);
        }
        self.artwork_album_hints
            .lock()
            .unwrap()
            .hints
            .retain(|old| old != &hint);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_hints_evict_old_albums_and_replace_existing_members() {
        let mut hints = ArtworkAlbumHints::default();
        let scope = AlbumScope {
            source_generation: 1,
            database: "library.db".into(),
            remote: None,
        };
        for index in 0..=MAX_ALBUM_HINTS {
            hints.remember(AlbumHint {
                scope: scope.clone(),
                album: index.to_string(),
                id: index as i64,
                source_path: format!("{index}.wav"),
                cache_path: format!("{index}.png").into(),
            });
        }
        assert_eq!(hints.hints.len(), MAX_ALBUM_HINTS);
        assert!(!hints.hints.iter().any(|hint| hint.album == "0"));
        let mut replacement = hints.hints.front().unwrap().clone();
        replacement.id = -7;
        hints.remember(replacement);
        assert_eq!(hints.hints.len(), MAX_ALBUM_HINTS);
        assert_eq!(hints.hints.back().unwrap().id, -7);
        assert_eq!(
            hints.hints.iter().filter(|hint| hint.album == "1").count(),
            1
        );
    }
}
