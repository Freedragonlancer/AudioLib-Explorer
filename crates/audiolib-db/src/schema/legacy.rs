// crates/audiolib-db/src/schema/legacy.rs
//! Historical playback layout used only while normalizing owned pre-v6 sources.
//! Current libraries and snapshots never create these objects.
use super::*;

pub(super) fn add_playback_schema(conn: &Connection) -> Result<()> {
    for (name, definition) in [
        ("last_played_at", "TEXT"),
        ("play_count", "INTEGER DEFAULT 0"),
    ] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('audio_files') WHERE name=?1 COLLATE NOCASE)",
            [name], |row| row.get(0),
        ).map_err(db_error)?;
        if !exists {
            ensure_column(conn, "audio_files", name, definition)?;
        }
    }
    conn.execute_batch("CREATE TABLE IF NOT EXISTS play_history (
        id INTEGER PRIMARY KEY, audio_file_id INTEGER NOT NULL REFERENCES audio_files(id) ON DELETE CASCADE,
        played_at TEXT NOT NULL, filename TEXT NOT NULL, file_path TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS idx_play_history_date ON play_history(played_at DESC,id DESC);
        CREATE INDEX IF NOT EXISTS idx_play_history_audio ON play_history(audio_file_id);
        CREATE INDEX IF NOT EXISTS idx_play_history_audio_file ON play_history(audio_file_id);")
        .map_err(db_error)
}
