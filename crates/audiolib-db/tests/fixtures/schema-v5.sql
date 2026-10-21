-- crates/audiolib-db/tests/fixtures/schema-v5.sql
-- Historical v5 external-content layout captured from the frozen research baseline.
-- Baseline file SHA256: 337d4bb898883a36a3980925e596844ba5effe22484a945a91bc55879736ae51
-- Schema only: no user records, paths, IDs, annotations or server secrets are copied.
-- Empty seed rows below are synthetic and deliberately retain completed rev 3.
CREATE TABLE audio_files (
 id INTEGER PRIMARY KEY, file_path TEXT NOT NULL UNIQUE, filename TEXT NOT NULL, file_size INTEGER, sample_rate INTEGER, bit_depth INTEGER, channels INTEGER, duration REAL, codec TEXT, bwf_description TEXT, bwf_originator TEXT, bwf_originator_reference TEXT, bwf_origination_date TEXT, bwf_origination_time TEXT, bwf_time_reference INTEGER, bwf_version INTEGER, bwf_umid TEXT, ixml_scene TEXT, ixml_take TEXT, ixml_tape TEXT, ixml_circled INTEGER, ixml_wild_track INTEGER, ixml_no_good INTEGER, peak_amplitude REAL, rms_level REAL, spectral_centroid REAL, zero_crossing_rate REAL, user_tags TEXT, user_rating INTEGER, user_color TEXT, user_notes TEXT, created_at TEXT, modified_at TEXT, last_played_at TEXT, play_count INTEGER DEFAULT 0, filename_tokenized TEXT, user_tags_tokenized TEXT, folder_id INTEGER, user_notes_tokenized TEXT, bwf_description_tokenized TEXT, ixml_scene_tokenized TEXT, ixml_take_tokenized TEXT, favorite INTEGER NOT NULL DEFAULT 0, source_mtime_ns INTEGER, metadata_json TEXT NOT NULL DEFAULT '{}', missing INTEGER NOT NULL DEFAULT 0, seen_scan_id INTEGER, path_key TEXT, source_tags_text TEXT NOT NULL DEFAULT '');
CREATE VIRTUAL TABLE "audio_search" USING fts5(
 filename,filename_tokenized,user_tags,user_tags_tokenized,
 bwf_description,bwf_description_tokenized,ixml_scene,ixml_scene_tokenized,
 ixml_take,ixml_take_tokenized,user_notes,user_notes_tokenized,
 content='audio_files',content_rowid='id',tokenize='unicode61 remove_diacritics 2');
CREATE TABLE audiolib_audio_id_sequence (
         singleton INTEGER PRIMARY KEY CHECK(singleton=1),
         high_water INTEGER NOT NULL CHECK(high_water>=0));
CREATE TABLE audiolib_schema_features (
         feature TEXT PRIMARY KEY, version INTEGER NOT NULL);
CREATE TABLE folder_closure (
 ancestor_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
 descendant_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
 depth INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(ancestor_id,descendant_id));
CREATE TABLE folders (
 id INTEGER PRIMARY KEY, folder_path TEXT NOT NULL UNIQUE, folder_name TEXT NOT NULL,
 parent_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
 depth INTEGER NOT NULL DEFAULT 0, file_count INTEGER NOT NULL DEFAULT 0,
 total_file_count INTEGER NOT NULL DEFAULT 0, total_duration REAL DEFAULT 0,
 total_size INTEGER DEFAULT 0, created_at TEXT, updated_at TEXT, last_scanned TEXT);
CREATE TABLE library_meta(
        key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE play_history (
        id INTEGER PRIMARY KEY, audio_file_id INTEGER NOT NULL REFERENCES audio_files(id) ON DELETE CASCADE,
        played_at TEXT NOT NULL, filename TEXT NOT NULL, file_path TEXT NOT NULL);
CREATE TABLE playlist_items (
 id INTEGER PRIMARY KEY, playlist_id INTEGER REFERENCES playlists(id),
 audio_file_id INTEGER REFERENCES audio_files(id), position INTEGER, added_at TEXT, notes TEXT);
CREATE TABLE playlists (
 id INTEGER PRIMARY KEY, name TEXT NOT NULL, description TEXT, created_at TEXT, modified_at TEXT);
CREATE TABLE project_references (
 id INTEGER PRIMARY KEY, audio_file_id INTEGER REFERENCES audio_files(id),
 project_path TEXT NOT NULL, project_type TEXT, reference_count INTEGER DEFAULT 1, last_seen TEXT);
CREATE TABLE scan_sessions (
 id INTEGER PRIMARY KEY, start_time TEXT, end_time TEXT, scan_path TEXT, scan_type TEXT,
 files_scanned INTEGER DEFAULT 0, files_added INTEGER DEFAULT 0,
 files_updated INTEGER DEFAULT 0, files_removed INTEGER DEFAULT 0,
 files_skipped INTEGER DEFAULT 0, errors INTEGER DEFAULT 0, error_details TEXT,
 status TEXT, errors_truncated INTEGER NOT NULL DEFAULT 0);
CREATE TABLE waveform_cache (
 id INTEGER PRIMARY KEY, audio_file_id INTEGER REFERENCES audio_files(id), zoom_level TEXT,
 samples_per_pixel INTEGER, min_values BLOB, max_values BLOB, rms_values BLOB,
 color_values BLOB, created_at TEXT);
CREATE INDEX idx_audio_files_folder ON audio_files(folder_id);
CREATE INDEX idx_audio_files_missing ON audio_files(missing) WHERE missing=1;
CREATE INDEX idx_audio_files_not_present ON audio_files(id) WHERE missing IS NOT 0;
CREATE INDEX idx_audio_files_path_key ON audio_files(path_key);
CREATE INDEX idx_audio_search_facets ON audio_files(id,missing,folder_id);
CREATE INDEX idx_audio_visible_color ON audio_files(user_color COLLATE NOCASE,id) WHERE missing=0 AND user_color IS NOT NULL;
CREATE INDEX idx_audio_visible_favorite ON audio_files(filename COLLATE NOCASE,id) WHERE missing=0 AND favorite=1;
CREATE INDEX idx_audio_visible_filter ON audio_files(sample_rate,duration,id) WHERE missing=0;
CREATE INDEX idx_audio_visible_folder_name ON audio_files(folder_id,filename COLLATE NOCASE,id) WHERE missing=0;
CREATE INDEX idx_audio_visible_format ON audio_files(bit_depth,channels) WHERE missing=0;
CREATE INDEX idx_audio_visible_format_name ON audio_files(bit_depth,filename COLLATE NOCASE,id) WHERE missing=0;
CREATE INDEX idx_audio_visible_id ON audio_files(id) WHERE missing=0;
CREATE INDEX idx_audio_visible_modified ON audio_files(modified_at,id) WHERE missing=0;
CREATE INDEX idx_audio_visible_modified_desc ON audio_files(modified_at DESC,id) WHERE missing=0;
CREATE INDEX idx_audio_visible_name ON audio_files(filename COLLATE NOCASE,id) WHERE missing=0;
CREATE INDEX idx_audio_visible_stats ON audio_files(folder_id,duration,file_size) WHERE missing=0;
CREATE INDEX idx_audio_visible_tagged_any ON audio_files(filename COLLATE NOCASE,id,user_tags,source_tags_text) WHERE missing=0 AND (COALESCE(trim(user_tags),'')<>'' OR source_tags_text<>'');
CREATE INDEX idx_audio_visible_tagged_folders
          ON audio_files(folder_id,id,user_tags,source_tags_text)
          WHERE missing=0 AND (COALESCE(trim(user_tags),'')<>'' OR source_tags_text<>'');
CREATE INDEX idx_closure_descendant ON folder_closure(descendant_id);
CREATE INDEX idx_duration ON audio_files(duration);
CREATE INDEX idx_folders_parent ON folders(parent_id);
CREATE INDEX idx_play_history_audio ON play_history(audio_file_id);
CREATE INDEX idx_play_history_audio_file ON play_history(audio_file_id);
CREATE INDEX idx_play_history_date ON play_history(played_at DESC,id DESC);
CREATE INDEX idx_playlist_items_audio ON playlist_items(audio_file_id);
CREATE INDEX idx_playlist_items_audio_file ON playlist_items(audio_file_id);
CREATE INDEX idx_project_references_audio ON project_references(audio_file_id);
CREATE INDEX idx_project_references_audio_file ON project_references(audio_file_id);
CREATE INDEX idx_sample_rate ON audio_files(sample_rate);
CREATE INDEX idx_waveform_cache_audio ON waveform_cache(audio_file_id);
CREATE INDEX idx_waveform_cache_audio_file ON waveform_cache(audio_file_id);
CREATE TRIGGER audio_files_ad AFTER DELETE ON audio_files BEGIN
 INSERT INTO audio_search(audio_search,rowid,filename,filename_tokenized,user_tags,user_tags_tokenized,
 bwf_description,bwf_description_tokenized,ixml_scene,ixml_scene_tokenized,
 ixml_take,ixml_take_tokenized,user_notes,user_notes_tokenized)
 VALUES('delete',old.id,old.filename,old.filename_tokenized,old.user_tags,old.user_tags_tokenized,
 old.bwf_description,old.bwf_description_tokenized,old.ixml_scene,old.ixml_scene_tokenized,
 old.ixml_take,old.ixml_take_tokenized,old.user_notes,old.user_notes_tokenized);
END;
CREATE TRIGGER audio_files_ai AFTER INSERT ON audio_files BEGIN
 INSERT INTO audio_search(rowid,filename,filename_tokenized,user_tags,user_tags_tokenized,
 bwf_description,bwf_description_tokenized,ixml_scene,ixml_scene_tokenized,
 ixml_take,ixml_take_tokenized,user_notes,user_notes_tokenized)
 VALUES(new.id,new.filename,new.filename_tokenized,new.user_tags,new.user_tags_tokenized,
 new.bwf_description,new.bwf_description_tokenized,new.ixml_scene,new.ixml_scene_tokenized,
 new.ixml_take,new.ixml_take_tokenized,new.user_notes,new.user_notes_tokenized);
END;
CREATE TRIGGER audio_files_au AFTER UPDATE OF filename,filename_tokenized,user_tags,user_tags_tokenized,
 bwf_description,bwf_description_tokenized,ixml_scene,ixml_scene_tokenized,
 ixml_take,ixml_take_tokenized,user_notes,user_notes_tokenized ON audio_files BEGIN
 INSERT INTO audio_search(audio_search,rowid,filename,filename_tokenized,user_tags,user_tags_tokenized,
 bwf_description,bwf_description_tokenized,ixml_scene,ixml_scene_tokenized,
 ixml_take,ixml_take_tokenized,user_notes,user_notes_tokenized)
 VALUES('delete',old.id,old.filename,old.filename_tokenized,old.user_tags,old.user_tags_tokenized,
 old.bwf_description,old.bwf_description_tokenized,old.ixml_scene,old.ixml_scene_tokenized,
 old.ixml_take,old.ixml_take_tokenized,old.user_notes,old.user_notes_tokenized);
 INSERT INTO audio_search(rowid,filename,filename_tokenized,user_tags,user_tags_tokenized,
 bwf_description,bwf_description_tokenized,ixml_scene,ixml_scene_tokenized,
 ixml_take,ixml_take_tokenized,user_notes,user_notes_tokenized)
 VALUES(new.id,new.filename,new.filename_tokenized,new.user_tags,new.user_tags_tokenized,
 new.bwf_description,new.bwf_description_tokenized,new.ixml_scene,new.ixml_scene_tokenized,
 new.ixml_take,new.ixml_take_tokenized,new.user_notes,new.user_notes_tokenized);
END;
CREATE TRIGGER audiolib_audio_id_high_water AFTER INSERT ON audio_files BEGIN
          UPDATE audiolib_audio_id_sequence SET high_water=MAX(high_water,new.id) WHERE singleton=1;
         END;
INSERT INTO audiolib_schema_features(feature,version) VALUES
 ('foreign_key_indexes',1),('search_indexes',1),('stable_audio_ids',1),('tokenization',3);
INSERT INTO audiolib_audio_id_sequence(singleton,high_water) VALUES(1,0);
INSERT INTO library_meta(key,value) VALUES('source_search_rev','2'),('source_tags_rev','2');
PRAGMA user_version=5;