# AudioLib Explorer - Technical Architecture Document

> Current runtime: Rust + Slint. Python examples here are retained as legacy design history; the Python implementation and its test scripts have been removed from the working tree and remain available in earlier committed versions. The original database and local config/audiolib_config.yaml remain read-only import inputs. Use [Rust Architecture](Rust_Architecture.md) and [Rust Validation](Rust_Validation.md) for current implementation and evidence; Appendix A remains authoritative terminology.

**Version:** 1.0
**Date:** 2025-11-23
**Status:** Architecture Specification
**Related Documents:**
- Product Requirements: `AudioLib_Explorer_PRD.md`
- Original Requirements: `AudioLib_Explorer Requirement.md`

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Technology Stack](#2-technology-stack)
3. [System Architecture](#3-system-architecture)
4. [Data Architecture](#4-data-architecture)
5. [Module Structure](#5-module-structure)
6. [Plugin Architecture](#6-plugin-architecture)
7. [Multi-Language Framework](#7-multi-language-framework)
8. [Performance Optimization](#8-performance-optimization)
9. [Client-Server Abstraction](#9-client-server-abstraction)
10. [Development Roadmap](#10-development-roadmap)

---

## 1. Architecture Overview

### 1.1 Design Principles

**1. Local-First, Cloud-Ready**
- Phase 1: Fully functional offline desktop application
- Phase 2: Transparent migration to client-server architecture
- Data provider abstraction enables seamless transition

**2. Performance at Scale**
- Target: 1M+ audio file records with sub-second search
- Multi-level caching for waveforms and metadata
- Lazy loading and progressive rendering

**3. Extensibility via Plugins**
- Core features built-in, advanced features as plugins
- Well-defined plugin API with sandboxing
- Community-driven ecosystem

**4. Modularity and Maintainability**
- Clean separation of concerns (MVC pattern)
- Service registry for dependency injection
- Clear module boundaries with defined interfaces

**5. Internationalization from Day One**
- No hardcoded UI text
- Locale-aware formatting and tokenization
- Right-to-left layout preparation (future)

### 1.2 Architectural Style

**Primary Pattern:** Model-View-Controller (MVC)

```
┌─────────────────────────────────────────────────────────────┐
│                     View Layer (GUI)                         │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────────────┐   │
│  │ Main Window  │  │ Waveform     │  │ Spectrum        │   │
│  │              │  │ Viewer       │  │ Analyzer        │   │
│  └──────────────┘  └──────────────┘  └─────────────────┘   │
└──────────────────────────┬──────────────────────────────────┘
                           │ Events / Commands
┌──────────────────────────▼──────────────────────────────────┐
│                   Controller Layer                           │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────────────┐   │
│  │ Scanner      │  │ Search       │  │ Audio Player    │   │
│  │ Controller   │  │ Controller   │  │ Controller      │   │
│  └──────────────┘  └──────────────┘  └─────────────────┘   │
└──────────────────────────┬──────────────────────────────────┘
                           │ Data Access
┌──────────────────────────▼──────────────────────────────────┐
│                     Model Layer (Data)                       │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────────────┐   │
│  │ AudioFile    │  │ Playlist     │  │ WaveformCache   │   │
│  │ (ORM)        │  │ (ORM)        │  │ (ORM)           │   │
│  └──────────────┘  └──────────────┘  └─────────────────┘   │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────▼──────────────────────────────────┐
│                  Data Provider Abstraction                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │  LocalDataProvider (SQLite) ◄─► RemoteDataProvider    │ │
│  └────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

### 1.3 Reference Architecture: Video_Explorer

AudioLib Explorer leverages proven patterns from the Video_Explorer project:

**Reusable Components:**
- Service Registry System (`src/core/services.py`)
- Plugin Architecture (`src/core/plugins.py`)
- Configuration Management (`src/utils/config.py`)
- DPI Scaling Utilities (`src/utils/dpi_scaling.py`)
- Database Management (`src/database/db_manager.py`)

**Adaptation Required:**
- Replace video-specific metadata with audio metadata (WAV, BWF, iXML)
- Replace video playback with audio playback (sounddevice)
- Replace video thumbnails with waveform caching
- Add full-text search (FTS5) for metadata

---

## 2. Technology Stack

### 2.1 Core Technologies

| Component | Technology | Version | Rationale |
|-----------|-----------|---------|-----------|
| **Language** | Python | 3.11+ | Proven in Video_Explorer, rich audio ecosystem, rapid development |
| **GUI Framework** | tkinter | Built-in | No external dependencies, mature, cross-platform |
| **Database** | SQLite | 3.35+ | Serverless, FTS5 support, proven at 1M+ scale |
| **ORM** | SQLAlchemy | 2.0+ | Type-safe, excellent SQLite support, industry standard |
| **Configuration** | Pydantic | 2.5+ | Runtime validation, type hints, JSON schema |
| **Logging** | loguru | 0.7+ | Clean API, colored output, rotation built-in |

### 2.2 Audio Processing Stack

| Library | Version | Purpose | Alternatives Considered |
|---------|---------|---------|------------------------|
| **soundfile** | 0.12+ | WAV file I/O, format-agnostic reading | wave (built-in, but limited) |
| **wavinfo** | 2.1+ | Broadcast WAV (BWF) metadata extraction | tinytag (less BWF support) |
| **python-sounddevice** | 0.4+ | High-quality audio playback | PyAudio (portability issues) |
| **librosa** | 0.10+ | Waveform/spectrum analysis, feature extraction | scipy (lower-level, no audio focus) |
| **numpy** | 1.24+ | Array processing, decimation, FFT | pandas (overkill for audio) |
| **matplotlib** | 3.7+ | Waveform/spectrum visualization in tkinter | plotly (heavyweight, web-oriented) |

**Excluded:**
- **pydub**: Requires ffmpeg dependency, overkill for WAV-only
- **aubio**: C dependency complexity, librosa is sufficient

### 2.3 Supporting Libraries

| Library | Purpose | Notes |
|---------|---------|-------|
| **TkinterDnD2** | Drag-and-drop file sharing | Windows OLE drag-and-drop |
| **pillow (PIL)** | Image processing for icons/thumbnails | May generate waveform as image for performance |
| **psutil** | System resource monitoring | Memory usage tracking, performance profiling |
| **pytest** | Unit testing framework | 70%+ code coverage target |
| **black** | Code formatting | PEP 8 compliance |

### 2.4 Dependency Management

**Package Manager:** pip with requirements.txt

```txt
# requirements.txt
SQLAlchemy==2.0.23
pydantic==2.5.2
loguru==0.7.2
soundfile==0.12.1
wavinfo==2.1.0
python-sounddevice==0.4.6
librosa==0.10.1
numpy==1.24.3
matplotlib==3.7.2
TkinterDnD2==0.3.0
pillow==10.1.0
psutil==5.9.6
pytest==7.4.3
black==23.11.0
```

**Version Pinning Strategy:**
- Pin major.minor versions (e.g., `librosa==0.10.1`)
- Test before upgrading dependencies
- Document breaking changes in CHANGELOG.md

### 2.5 Deployment Stack

**Development:**
- Python 3.11.6 (official release)
- Visual Studio Code with Python extension
- Git for version control (GitHub)

**Distribution:**
- **Windows:** PyInstaller → Single .exe or NSIS installer
- **macOS (future):** py2app → .app bundle
- **Linux (future):** AppImage or .deb package

---

## 3. System Architecture

### 3.1 High-Level Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                        PRESENTATION LAYER                            │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │  Main Window (tkinter.Tk)                                     │  │
│  │  ├─ Menu Bar (File, Edit, View, Tools, Help)                 │  │
│  │  ├─ Toolbar (Scan, Refresh, Search)                          │  │
│  │  ├─ Folder Tree Panel (ttk.Treeview)                         │  │
│  │  ├─ Results List Panel (ttk.Treeview + Scrollbar)            │  │
│  │  ├─ Waveform Viewer Panel (matplotlib FigureCanvasTkAgg)     │  │
│  │  ├─ Spectrum Analyzer Panel (matplotlib FigureCanvasTkAgg)   │  │
│  │  ├─ Audio Player Controls (ttk.Frame + custom widgets)       │  │
│  │  └─ Status Bar (ttk.Label)                                   │  │
│  └───────────────────────────────────────────────────────────────┘  │
│         │                  │                  │                      │
│         ▼                  ▼                  ▼                      │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────────┐                │
│  │ i18n        │  │ Layout      │  │ DPI Scaling  │                │
│  │ Manager     │  │ Manager     │  │ Manager      │                │
│  └─────────────┘  └─────────────┘  └──────────────┘                │
└─────────────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────▼───────────────────────────────────────┐
│                        BUSINESS LOGIC LAYER                          │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌────────────┐ │
│  │ Scanner     │  │ Search      │  │ Audio       │  │ Waveform   │ │
│  │ Service     │  │ Engine      │  │ Player      │  │ Generator  │ │
│  └─────────────┘  └─────────────┘  └─────────────┘  └────────────┘ │
│                                                                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌────────────┐ │
│  │ Metadata    │  │ Plugin      │  │ Config      │  │ Service    │ │
│  │ Extractor   │  │ Manager     │  │ Manager     │  │ Registry   │ │
│  └─────────────┘  └─────────────┘  └─────────────┘  └────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────▼───────────────────────────────────────┐
│                         DATA ACCESS LAYER                            │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │             Data Provider Abstraction Interface             │    │
│  │  - search(query, filters) → List[AudioFile]                │    │
│  │  - get_metadata(file_id) → AudioMetadata                    │    │
│  │  - scan_directory(path) → ScanResult                        │    │
│  │  - update_metadata(file_id, metadata) → bool                │    │
│  └─────────────────────────────────────────────────────────────┘    │
│          │                                      │                    │
│          ▼                                      ▼                    │
│  ┌──────────────────┐                  ┌──────────────────┐         │
│  │ LocalDataProvider│                  │RemoteDataProvider│         │
│  │  (SQLite/FTS5)   │                  │   (REST Client)  │         │
│  └──────────────────┘                  └──────────────────┘         │
└─────────────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────▼───────────────────────────────────────┐
│                         PERSISTENCE LAYER                            │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐  │
│  │ SQLite Database  │  │ Waveform Cache   │  │ Configuration    │  │
│  │ - audio_files    │  │ (BLOB storage)   │  │ (JSON files)     │  │
│  │ - audio_search   │  │                  │  │                  │  │
│  │ - playlists      │  │                  │  │                  │  │
│  │ - scan_sessions  │  │                  │  │                  │  │
│  └──────────────────┘  └──────────────────┘  └──────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.2 Threading Model

**Main Thread:** GUI event loop (tkinter mainloop)

**Background Threads:**
1. **Scanner Thread:** Directory scanning and metadata extraction
2. **Waveform Generator Thread:** Waveform cache generation
3. **Audio Playback Thread:** Managed by python-sounddevice (internal)

**Thread Communication:**
- **Queue-based messaging:** `queue.Queue` for thread-safe communication
- **Callback pattern:** Background threads invoke GUI callbacks via `root.after()`
- **Thread synchronization:** `threading.Lock` for database access

```python
# Thread-safe GUI update pattern
class ScannerService:
    def scan_directory(self, path: str, progress_callback):
        def scan_worker():
            for i, file in enumerate(audio_files):
                metadata = extract_metadata(file)
                self.db.add(metadata)

                # Update GUI safely
                self.root.after(0, progress_callback, i, len(audio_files))

        thread = threading.Thread(target=scan_worker, daemon=True)
        thread.start()
```

### 3.3 Process Model

**Single Process Architecture (Phase 1):**
- All components run in one Python process
- Plugin isolation via exception handling (not separate processes)

**Multi-Process Considerations (Future):**
- Waveform generation could move to separate worker processes (multiprocessing)
- Plugin sandboxing via subprocess.Popen()

---

## 4. Data Architecture

### 4.1 Complete Database Schema (SQLite)

#### 4.1.1 Audio Files Table

```sql
CREATE TABLE audio_files (
    -- Primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- File information
    filename VARCHAR(500) NOT NULL,
    file_path TEXT NOT NULL UNIQUE,
    file_size INTEGER NOT NULL,
    file_hash VARCHAR(64),  -- SHA-256 for duplicate detection

    -- Standard WAV metadata
    sample_rate INTEGER NOT NULL,  -- 44100, 48000, 96000, 192000
    bit_depth INTEGER NOT NULL,    -- 16, 24, 32
    channels INTEGER NOT NULL,      -- 1=mono, 2=stereo, 6=5.1, 8=7.1
    duration REAL NOT NULL,         -- seconds with millisecond precision

    -- Broadcast Wave Format (BWF) extensions
    bext_description TEXT,              -- BWF description field
    bext_originator VARCHAR(255),       -- Who created the file
    bext_originator_reference VARCHAR(255),  -- Production reference
    bext_originator_date VARCHAR(10),   -- YYYY-MM-DD
    bext_originator_time VARCHAR(8),    -- HH:MM:SS
    bext_time_reference BIGINT,         -- SMPTE timecode
    bext_coding_history TEXT,           -- Processing history

    -- iXML metadata (production workflow)
    ixml_project VARCHAR(255),          -- Project name
    ixml_scene VARCHAR(255),            -- Scene identifier
    ixml_take VARCHAR(255),             -- Take number
    ixml_tape VARCHAR(255),             -- Tape/card identifier
    ixml_file_uid VARCHAR(255),         -- Unique file identifier

    -- Computed audio characteristics
    peak_amplitude REAL,                -- Maximum amplitude (0.0-1.0)
    rms_level REAL,                     -- RMS loudness
    spectral_centroid REAL,             -- Brightness measure (Hz)
    zero_crossing_rate REAL,            -- Noisiness indicator

    -- Timestamps
    created_date DATETIME NOT NULL,     -- File creation time
    modified_date DATETIME NOT NULL,    -- File modification time
    scanned_date DATETIME DEFAULT CURRENT_TIMESTAMP,  -- When added to DB
    last_accessed DATETIME,             -- Last played/opened

    -- User annotations
    user_rating INTEGER CHECK(user_rating BETWEEN 1 AND 5),
    is_favorite BOOLEAN DEFAULT 0,
    user_tags TEXT,                     -- JSON array: ["explosion", "outdoor"]
    user_notes TEXT,                    -- Free-form text notes

    -- Project cross-reference (addresses PRD review critical issue #1)
    project_references TEXT,            -- JSON array of project file paths

    -- Metadata
    import_source VARCHAR(255),         -- Original library/source

    -- Indexes
    CONSTRAINT file_path_unique UNIQUE (file_path)
);

-- Performance indexes
CREATE INDEX idx_filename ON audio_files(filename);
CREATE INDEX idx_sample_rate ON audio_files(sample_rate);
CREATE INDEX idx_bit_depth ON audio_files(bit_depth);
CREATE INDEX idx_channels ON audio_files(channels);
CREATE INDEX idx_duration ON audio_files(duration);
CREATE INDEX idx_created_date ON audio_files(created_date);
CREATE INDEX idx_is_favorite ON audio_files(is_favorite);
CREATE INDEX idx_user_rating ON audio_files(user_rating);
CREATE INDEX idx_file_hash ON audio_files(file_hash);  -- Duplicate detection

-- Composite indexes for common queries
CREATE INDEX idx_sample_rate_depth ON audio_files(sample_rate, bit_depth);
CREATE INDEX idx_duration_channels ON audio_files(duration, channels);
```

#### 4.1.2 Full-Text Search Table (FTS5)

```sql
-- Virtual table for blazing-fast full-text search
CREATE VIRTUAL TABLE audio_search USING fts5(
    filename,
    user_tags,
    user_notes,
    bext_description,
    bext_originator,
    ixml_project,
    ixml_scene,
    file_path,

    -- Link to main table
    content='audio_files',
    content_rowid='id',

    -- Tokenization configuration
    tokenize='porter unicode61 remove_diacritics 2'
    -- porter: Stemming (explosion → explod)
    -- unicode61: Proper Unicode handling for Chinese
    -- remove_diacritics: café = cafe
);

-- Triggers to keep FTS index synchronized
CREATE TRIGGER audio_search_ai AFTER INSERT ON audio_files BEGIN
  INSERT INTO audio_search(rowid, filename, user_tags, user_notes,
                           bext_description, bext_originator, ixml_project,
                           ixml_scene, file_path)
  VALUES (new.id, new.filename, new.user_tags, new.user_notes,
          new.bext_description, new.bext_originator, new.ixml_project,
          new.ixml_scene, new.file_path);
END;

CREATE TRIGGER audio_search_ad AFTER DELETE ON audio_files BEGIN
  DELETE FROM audio_search WHERE rowid = old.id;
END;

CREATE TRIGGER audio_search_au AFTER UPDATE ON audio_files BEGIN
  UPDATE audio_search SET
    filename = new.filename,
    user_tags = new.user_tags,
    user_notes = new.user_notes,
    bext_description = new.bext_description,
    bext_originator = new.bext_originator,
    ixml_project = new.ixml_project,
    ixml_scene = new.ixml_scene,
    file_path = new.file_path
  WHERE rowid = old.id;
END;
```

#### 4.1.3 Waveform Cache Table

```sql
CREATE TABLE waveform_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    audio_file_id INTEGER NOT NULL,

    -- Zoom level identifier
    zoom_level VARCHAR(20) NOT NULL,  -- 'overview', 'minute', 'second'

    -- Cached waveform data (NumPy array pickled)
    waveform_data BLOB NOT NULL,

    -- Decimation parameters
    samples_per_pixel INTEGER NOT NULL,

    -- Cache metadata
    generated_at DATETIME DEFAULT CURRENT_TIMESTAMP,

    -- Foreign key
    FOREIGN KEY (audio_file_id) REFERENCES audio_files(id) ON DELETE CASCADE,

    -- Unique constraint: one cache per file per zoom level
    CONSTRAINT unique_cache UNIQUE (audio_file_id, zoom_level)
);

CREATE INDEX idx_waveform_cache_file ON waveform_cache(audio_file_id);
```

#### 4.1.4 Playlists Table

```sql
CREATE TABLE playlists (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name VARCHAR(255) NOT NULL,
    description TEXT,
    created_date DATETIME DEFAULT CURRENT_TIMESTAMP,
    modified_date DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE playlist_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id INTEGER NOT NULL,
    audio_file_id INTEGER NOT NULL,
    position INTEGER NOT NULL,  -- Order in playlist

    FOREIGN KEY (playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
    FOREIGN KEY (audio_file_id) REFERENCES audio_files(id) ON DELETE CASCADE,

    CONSTRAINT unique_playlist_item UNIQUE (playlist_id, audio_file_id)
);

CREATE INDEX idx_playlist_items_playlist ON playlist_items(playlist_id);
CREATE INDEX idx_playlist_items_position ON playlist_items(playlist_id, position);
```

#### 4.1.5 Project References Table

**Purpose**: Track which DAW project files reference which audio library files for cross-reference queries and deletion safety.

```sql
CREATE TABLE project_references (
    -- Primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Foreign key to audio file
    audio_file_id INTEGER NOT NULL,

    -- Project file information
    project_path TEXT NOT NULL,           -- Full path to DAW project file
    project_name VARCHAR(500) NOT NULL,   -- Project filename (e.g., "Game_Boss_Battle.ptx")
    project_type VARCHAR(50) NOT NULL,    -- 'protools', 'reaper', 'ableton', 'flstudio', etc.

    -- Reference details
    reference_path TEXT NOT NULL,         -- Path as stored in project (may be relative)
    is_relative_path BOOLEAN DEFAULT 0,   -- Whether reference uses relative path

    -- Tracking metadata
    last_verified DATETIME DEFAULT CURRENT_TIMESTAMP,  -- Last verification time
    project_modified_date DATETIME,       -- Project file's last modified timestamp
    reference_valid BOOLEAN DEFAULT 1,    -- Whether reference path still resolves

    FOREIGN KEY (audio_file_id) REFERENCES audio_files(id) ON DELETE CASCADE
);

-- Indexes for efficient cross-reference queries
CREATE INDEX idx_project_audio_file ON project_references(audio_file_id);
CREATE INDEX idx_project_path ON project_references(project_path);
CREATE INDEX idx_project_type ON project_references(project_type);
CREATE INDEX idx_reference_valid ON project_references(reference_valid);
```

**Query Examples**:

```sql
-- Find all projects that reference a specific audio file
SELECT project_name, project_path, last_verified
FROM project_references
WHERE audio_file_id = 12345
  AND reference_valid = 1;

-- Find all audio files used in a specific project
SELECT af.filename, af.file_path, pr.reference_path
FROM audio_files af
JOIN project_references pr ON af.id = pr.audio_file_id
WHERE pr.project_path = '/projects/Game_Boss_Battle.ptx';

-- Find all unused audio files (not referenced by any project)
SELECT af.id, af.filename, af.file_path
FROM audio_files af
LEFT JOIN project_references pr ON af.id = pr.audio_file_id
WHERE pr.id IS NULL;

-- Get project reference count for search results
SELECT af.filename, COUNT(pr.id) as project_count
FROM audio_files af
LEFT JOIN project_references pr ON af.id = pr.audio_file_id
WHERE af.filename LIKE '%explosion%'
GROUP BY af.id;
```

#### 4.1.6 Scan Sessions Table

```sql
CREATE TABLE scan_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scan_path TEXT NOT NULL,
    scan_type VARCHAR(20) NOT NULL,  -- 'full', 'incremental', 'verify'

    -- Statistics
    files_scanned INTEGER NOT NULL DEFAULT 0,
    files_added INTEGER NOT NULL DEFAULT 0,
    files_updated INTEGER NOT NULL DEFAULT 0,
    files_failed INTEGER NOT NULL DEFAULT 0,

    -- Timing
    started_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    completed_at DATETIME,
    duration_seconds REAL,

    -- Status
    status VARCHAR(20) NOT NULL,  -- 'running', 'completed', 'cancelled', 'failed'
    error_message TEXT
);

CREATE INDEX idx_scan_sessions_started ON scan_sessions(started_at);
```

#### 4.1.7 Database Configuration Table

```sql
CREATE TABLE database_metadata (
    key VARCHAR(255) PRIMARY KEY,
    value TEXT NOT NULL
);

-- Seed initial metadata
INSERT INTO database_metadata (key, value) VALUES
    ('version', '1.0'),
    ('created_at', datetime('now')),
    ('schema_version', '1');
```

### 4.2 SQLAlchemy ORM Models

```python
# src/database/models.py

from sqlalchemy import Column, Integer, String, Float, Boolean, Text, DateTime, ForeignKey, LargeBinary
from sqlalchemy.orm import relationship, declarative_base
from sqlalchemy.sql import func
import json

Base = declarative_base()

class AudioFile(Base):
    __tablename__ = 'audio_files'

    # Primary key
    id = Column(Integer, primary_key=True, autoincrement=True)

    # File information
    filename = Column(String(500), nullable=False, index=True)
    file_path = Column(Text, nullable=False, unique=True)
    file_size = Column(Integer, nullable=False)
    file_hash = Column(String(64), index=True)

    # Standard WAV metadata
    sample_rate = Column(Integer, nullable=False, index=True)
    bit_depth = Column(Integer, nullable=False, index=True)
    channels = Column(Integer, nullable=False, index=True)
    duration = Column(Float, nullable=False, index=True)

    # BWF extensions
    bext_description = Column(Text)
    bext_originator = Column(String(255))
    bext_originator_reference = Column(String(255))
    bext_originator_date = Column(String(10))
    bext_originator_time = Column(String(8))
    bext_time_reference = Column(Integer)
    bext_coding_history = Column(Text)

    # iXML metadata
    ixml_project = Column(String(255))
    ixml_scene = Column(String(255))
    ixml_take = Column(String(255))
    ixml_tape = Column(String(255))
    ixml_file_uid = Column(String(255))

    # Computed audio characteristics
    peak_amplitude = Column(Float)
    rms_level = Column(Float)
    spectral_centroid = Column(Float)
    zero_crossing_rate = Column(Float)

    # Timestamps
    created_date = Column(DateTime, nullable=False)
    modified_date = Column(DateTime, nullable=False)
    scanned_date = Column(DateTime, default=func.now())
    last_accessed = Column(DateTime)

    # User annotations
    user_rating = Column(Integer)
    is_favorite = Column(Boolean, default=False, index=True)
    user_tags = Column(Text)  # JSON array
    user_notes = Column(Text)
    project_references = Column(Text)  # JSON array
    import_source = Column(String(255))

    # Relationships
    waveform_caches = relationship("WaveformCache", back_populates="audio_file", cascade="all, delete-orphan")
    playlist_items = relationship("PlaylistItem", back_populates="audio_file", cascade="all, delete-orphan")

    def __repr__(self):
        return f"<AudioFile(id={self.id}, filename='{self.filename}')>"

    @property
    def tags_list(self):
        """Parse user_tags JSON to list"""
        if self.user_tags:
            return json.loads(self.user_tags)
        return []

    @tags_list.setter
    def tags_list(self, tags):
        """Store tags as JSON"""
        self.user_tags = json.dumps(tags)

    @property
    def projects_list(self):
        """Parse project_references JSON to list"""
        if self.project_references:
            return json.loads(self.project_references)
        return []

    @projects_list.setter
    def projects_list(self, projects):
        """Store project references as JSON"""
        self.project_references = json.dumps(projects)


class ProjectReference(Base):
    """
    Tracks which DAW project files reference which audio library files.
    Enables cross-reference queries and deletion safety warnings.
    """
    __tablename__ = 'project_references'

    # Primary key
    id = Column(Integer, primary_key=True, autoincrement=True)

    # Foreign key to audio file
    audio_file_id = Column(Integer, ForeignKey('audio_files.id', ondelete='CASCADE'),
                          nullable=False, index=True)

    # Project file information
    project_path = Column(Text, nullable=False, index=True)  # Full path to DAW project
    project_name = Column(String(500), nullable=False)       # Project filename
    project_type = Column(String(50), nullable=False)        # 'protools', 'reaper', 'ableton', etc.

    # Reference details
    reference_path = Column(Text, nullable=False)            # Path as stored in project (may be relative)
    is_relative_path = Column(Boolean, default=False)        # Whether reference uses relative path

    # Tracking metadata
    last_verified = Column(DateTime, default=func.now())     # Last time we verified this reference exists
    project_modified_date = Column(DateTime)                 # Project file's last modified timestamp
    reference_valid = Column(Boolean, default=True)          # Whether reference path still resolves

    # Relationship
    audio_file = relationship("AudioFile", backref="project_refs")

    def __repr__(self):
        return f"<ProjectReference(audio_id={self.audio_file_id}, project='{self.project_name}')>"

    @property
    def is_stale(self):
        """Check if reference verification is older than 24 hours"""
        from datetime import datetime, timedelta
        if not self.last_verified:
            return True
        return datetime.now() - self.last_verified > timedelta(hours=24)


class WaveformCache(Base):
    __tablename__ = 'waveform_cache'

    id = Column(Integer, primary_key=True, autoincrement=True)
    audio_file_id = Column(Integer, ForeignKey('audio_files.id', ondelete='CASCADE'), nullable=False)
    zoom_level = Column(String(20), nullable=False)
    waveform_data = Column(LargeBinary, nullable=False)
    samples_per_pixel = Column(Integer, nullable=False)
    generated_at = Column(DateTime, default=func.now())

    # Relationship
    audio_file = relationship("AudioFile", back_populates="waveform_caches")

    def __repr__(self):
        return f"<WaveformCache(file_id={self.audio_file_id}, zoom={self.zoom_level})>"


class Playlist(Base):
    __tablename__ = 'playlists'

    id = Column(Integer, primary_key=True, autoincrement=True)
    name = Column(String(255), nullable=False)
    description = Column(Text)
    created_date = Column(DateTime, default=func.now())
    modified_date = Column(DateTime, default=func.now(), onupdate=func.now())

    # Relationships
    items = relationship("PlaylistItem", back_populates="playlist", cascade="all, delete-orphan", order_by="PlaylistItem.position")

    def __repr__(self):
        return f"<Playlist(id={self.id}, name='{self.name}')>"


class PlaylistItem(Base):
    __tablename__ = 'playlist_items'

    id = Column(Integer, primary_key=True, autoincrement=True)
    playlist_id = Column(Integer, ForeignKey('playlists.id', ondelete='CASCADE'), nullable=False)
    audio_file_id = Column(Integer, ForeignKey('audio_files.id', ondelete='CASCADE'), nullable=False)
    position = Column(Integer, nullable=False)

    # Relationships
    playlist = relationship("Playlist", back_populates="items")
    audio_file = relationship("AudioFile", back_populates="playlist_items")

    def __repr__(self):
        return f"<PlaylistItem(playlist={self.playlist_id}, file={self.audio_file_id}, pos={self.position})>"


class ScanSession(Base):
    __tablename__ = 'scan_sessions'

    id = Column(Integer, primary_key=True, autoincrement=True)
    scan_path = Column(Text, nullable=False)
    scan_type = Column(String(20), nullable=False)

    files_scanned = Column(Integer, default=0)
    files_added = Column(Integer, default=0)
    files_updated = Column(Integer, default=0)
    files_failed = Column(Integer, default=0)

    started_at = Column(DateTime, default=func.now())
    completed_at = Column(DateTime)
    duration_seconds = Column(Float)

    status = Column(String(20), nullable=False)
    error_message = Column(Text)

    def __repr__(self):
        return f"<ScanSession(id={self.id}, path='{self.scan_path}', status='{self.status}')>"
```

### 4.3 Data Flow Diagrams

#### Scan Directory Flow

```
User clicks "Scan Directory"
        │
        ▼
┌───────────────────────┐
│ Scanner Controller    │
│ - Validate path       │
│ - Create ScanSession  │
└───────┬───────────────┘
        │
        ▼
┌───────────────────────┐
│ Background Thread     │
│ - Walk directory tree │
│ - Filter WAV files    │
└───────┬───────────────┘
        │
        ▼ (for each file)
┌───────────────────────┐
│ Metadata Extractor    │
│ - Read WAV headers    │
│ - Extract BWF/iXML    │
│ - Calculate hash      │
└───────┬───────────────┘
        │
        ▼
┌───────────────────────┐
│ Data Provider         │
│ - Check if exists     │
│ - Insert/Update DB    │
│ - Update FTS index    │
└───────┬───────────────┘
        │
        ▼
┌───────────────────────┐
│ GUI Update (callback) │
│ - Update progress bar │
│ - Update file count   │
└───────────────────────┘
```

#### Search Query Flow

```
User types search query
        │
        ▼
┌───────────────────────┐
│ Debounce (300ms)      │
│ - Wait for user stop  │
└───────┬───────────────┘
        │
        ▼
┌───────────────────────┐
│ Search Controller     │
│ - Parse query         │
│ - Apply filters       │
└───────┬───────────────┘
        │
        ▼
┌───────────────────────┐
│ Search Engine         │
│ - FTS5 query          │
│ - BM25 ranking        │
│ - Limit 1000 results  │
└───────┬───────────────┘
        │
        ▼
┌───────────────────────┐
│ Data Provider         │
│ - Execute SQL         │
│ - Hydrate ORM models  │
└───────┬───────────────┘
        │
        ▼
┌───────────────────────┐
│ Results List View     │
│ - Clear previous      │
│ - Populate table      │
│ - Update status       │
└───────────────────────┘
```

### 4.4 Database Migrations Strategy

#### 4.4.1 Migration Framework

**Tool:** Alembic (SQLAlchemy's official migration tool)

**Why Alembic:**
- Native SQLAlchemy integration
- Auto-generates migrations from model changes
- Support for upgrade/downgrade operations
- Production-tested and widely used

#### 4.4.2 Directory Structure

```
src/database/migrations/
├── alembic.ini              # Alembic configuration
├── env.py                   # Migration environment setup
├── script.py.mako           # Migration script template
└── versions/                # Migration version files
    ├── 001_initial_schema.py
    ├── 002_add_project_references.py
    ├── 003_add_waveform_cache_ttl.py
    └── 004_add_chinese_tokenizer.py
```

#### 4.4.3 Migration Script Example

```python
# versions/002_add_project_references.py
"""Add project_references column for cross-reference tracking

Revision ID: 002
Revises: 001
Create Date: 2025-11-24 10:00:00
"""
from alembic import op
import sqlalchemy as sa

# Revision identifiers
revision = '002'
down_revision = '001'
branch_labels = None
depends_on = None

def upgrade():
    """Add project_references column to audio_files table"""
    op.add_column('audio_files',
        sa.Column('project_references', sa.Text(), nullable=True))

    # Add index for faster project lookups
    op.create_index(
        'idx_project_references',
        'audio_files',
        ['project_references'],
        postgresql_using='gin',  # For future PostgreSQL support
        sqlite_where='project_references IS NOT NULL'
    )

def downgrade():
    """Remove project_references column"""
    op.drop_index('idx_project_references', 'audio_files')
    op.drop_column('audio_files', 'project_references')
```

#### 4.4.4 Database Version Tracking

**Schema Version Table:**

```sql
CREATE TABLE alembic_version (
    version_num VARCHAR(32) PRIMARY KEY
);

-- Example: Current version
INSERT INTO alembic_version (version_num) VALUES ('002');
```

**Version Check on Startup:**

```python
# src/database/db_manager.py

from alembic import command
from alembic.config import Config
from alembic.script import ScriptDirectory
from alembic.runtime.migration import MigrationContext

def check_and_migrate_database(engine):
    """Check database version and apply pending migrations"""
    alembic_cfg = Config("src/database/migrations/alembic.ini")

    # Get current database version
    with engine.connect() as connection:
        context = MigrationContext.configure(connection)
        current_rev = context.get_current_revision()

    # Get latest available version
    script = ScriptDirectory.from_config(alembic_cfg)
    head_rev = script.get_current_head()

    if current_rev != head_rev:
        logger.info(f"Database migration needed: {current_rev} → {head_rev}")

        # Ask user for confirmation (or auto-migrate in non-interactive mode)
        if confirm_migration_dialog(current_rev, head_rev):
            # Backup database before migration
            backup_database(engine.url.database)

            # Run migrations
            command.upgrade(alembic_cfg, "head")
            logger.info("Database migration completed successfully")
        else:
            logger.warning("Migration cancelled by user")
            return False
    else:
        logger.debug(f"Database is up to date (version {current_rev})")

    return True
```

#### 4.4.5 Migration Workflow

**Development Workflow:**

1. **Modify ORM Models:**
```python
# src/database/models.py
class AudioFile(Base):
    # Add new field
    spectral_flux = Column(Float)  # New audio feature
```

2. **Auto-Generate Migration:**
```bash
cd src/database/migrations
alembic revision --autogenerate -m "Add spectral_flux feature"
```

3. **Review Generated Migration:**
```python
# Alembic auto-generated file
def upgrade():
    op.add_column('audio_files',
        sa.Column('spectral_flux', sa.Float(), nullable=True))
```

4. **Test Migration:**
```bash
# Upgrade
alembic upgrade head

# Test rollback
alembic downgrade -1

# Re-upgrade
alembic upgrade head
```

5. **Commit Migration File:**
```bash
git add versions/005_add_spectral_flux.py
git commit -m "Add spectral_flux audio feature"
```

#### 4.4.6 Production Migration Strategy

**Pre-Migration Checklist:**
- [ ] Backup database (automatic via `backup_database()`)
- [ ] Test migration on copy of production database
- [ ] Verify rollback works
- [ ] Check migration execution time (should be <1 minute)
- [ ] Notify users if downtime required

**Migration Execution:**

```python
# src/main.py

def main():
    logger.info("AudioLib Explorer starting...")

    # Initialize database connection
    engine = create_optimized_engine(get_config().database.path)

    # Check and apply migrations (with backup)
    migration_success = check_and_migrate_database(engine)

    if not migration_success:
        logger.error("Database migration failed or cancelled")
        sys.exit(1)

    # Continue application startup
    app = MainWindow()
    app.mainloop()
```

#### 4.4.7 Handling Migration Failures

**Failure Recovery:**

```python
def check_and_migrate_database(engine):
    """Migrate database with error handling and recovery"""
    backup_path = None

    try:
        # Create backup before migration
        backup_path = backup_database(engine.url.database)
        logger.info(f"Database backed up to {backup_path}")

        # Apply migrations
        command.upgrade(alembic_cfg, "head")

        return True

    except Exception as e:
        logger.error(f"Migration failed: {e}", exc_info=True)

        # Ask user: Restore backup or Continue with current state?
        if backup_path and restore_backup_dialog():
            restore_database(backup_path, engine.url.database)
            logger.info("Database restored from backup")

        return False
```

#### 4.4.8 Alembic Configuration

**alembic.ini:**

```ini
[alembic]
script_location = src/database/migrations
sqlalchemy.url = sqlite:///database/default.db

# Logging
[loggers]
keys = root,sqlalchemy,alembic

[handlers]
keys = console

[formatters]
keys = generic

[logger_root]
level = INFO
handlers = console

[logger_sqlalchemy]
level = WARN
handlers =
qualname = sqlalchemy.engine

[logger_alembic]
level = INFO
handlers =
qualname = alembic

[handler_console]
class = StreamHandler
args = (sys.stderr,)
level = NOTSET
formatter = generic

[formatter_generic]
format = %(levelname)-5.5s [%(name)s] %(message)s
```

**env.py (Migration Environment):**

```python
# src/database/migrations/env.py

from logging.config import fileConfig
from sqlalchemy import engine_from_config, pool
from alembic import context
import sys
from pathlib import Path

# Add project root to path
sys.path.insert(0, str(Path(__file__).parents[3]))

from src.database.models import Base
from src.utils.config import get_config

# Alembic Config object
config = context.config

# Interpret the config file for Python logging
if config.config_file_name is not None:
    fileConfig(config.config_file_name)

# Add your model's MetaData object here for 'autogenerate' support
target_metadata = Base.metadata

def run_migrations_online():
    """Run migrations in 'online' mode (connected to database)"""
    # Get database URL from app config
    db_url = f"sqlite:///{get_config().database.path}"

    connectable = engine_from_config(
        config.get_section(config.config_ini_section),
        prefix="sqlalchemy.",
        poolclass=pool.NullPool,
        url=db_url,
    )

    with connectable.connect() as connection:
        context.configure(
            connection=connection,
            target_metadata=target_metadata,
            render_as_batch=True,  # Required for SQLite ALTER TABLE support
        )

        with context.begin_transaction():
            context.run_migrations()

if context.is_offline_mode():
    raise Exception("Offline mode not supported")
else:
    run_migrations_online()
```

---

## 5. Module Structure

### 5.1 Project Directory Layout

```
AudioLib_Explorer/
├── src/
│   ├── __init__.py
│   ├── main.py                      # Application entry point
│   │
│   ├── core/                        # Core utilities (from Video_Explorer)
│   │   ├── __init__.py
│   │   ├── services.py              # Service registry and DI
│   │   ├── plugins.py               # Plugin management system
│   │   └── exceptions.py            # Custom exceptions
│   │
│   ├── database/                    # Data layer
│   │   ├── __init__.py
│   │   ├── models.py                # SQLAlchemy ORM models
│   │   ├── db_manager.py            # Database connection and management
│   │   ├── data_provider.py         # Data provider abstraction
│   │   └── migrations/              # Database migrations (future)
│   │
│   ├── gui/                         # Presentation layer
│   │   ├── __init__.py
│   │   ├── main_window.py           # Main application window
│   │   ├── components/              # Reusable GUI components
│   │   │   ├── __init__.py
│   │   │   ├── folder_tree_panel.py
│   │   │   ├── results_list_view.py
│   │   │   ├── waveform_viewer.py
│   │   │   ├── spectrum_analyzer.py
│   │   │   ├── audio_player_controls.py
│   │   │   └── filter_sidebar.py
│   │   ├── dialogs/                 # Dialog windows
│   │   │   ├── scan_dialog.py
│   │   │   ├── preferences_dialog.py
│   │   │   └── about_dialog.py
│   │   └── styles.py                # UI styling constants
│   │
│   ├── scanner/                     # Audio library scanning
│   │   ├── __init__.py
│   │   ├── scanner_service.py       # Main scanning logic
│   │   ├── metadata_extractor.py    # WAV metadata extraction
│   │   └── file_utils.py            # File system utilities
│   │
│   ├── search/                      # Search and filtering
│   │   ├── __init__.py
│   │   ├── search_engine.py         # FTS5 search implementation
│   │   ├── query_parser.py          # Parse search syntax
│   │   └── filters.py               # Filter logic
│   │
│   ├── audio/                       # Audio processing
│   │   ├── __init__.py
│   │   ├── player.py                # Audio playback
│   │   ├── waveform_renderer.py     # Waveform generation
│   │   ├── spectrum_analyzer.py     # FFT and spectrum analysis
│   │   └── audio_utils.py           # Audio processing utilities
│   │
│   ├── i18n/                        # Internationalization
│   │   ├── __init__.py
│   │   ├── translations.py          # Translation manager
│   │   └── locales/                 # Translation files
│   │       ├── en.json              # English
│   │       └── cn.json              # Simplified Chinese
│   │
│   ├── plugins/                     # Plugin system
│   │   ├── __init__.py
│   │   ├── base.py                  # Plugin base classes
│   │   └── builtin/                 # Built-in plugins
│   │       ├── color_waveform/
│   │       └── advanced_spectrum/
│   │
│   └── utils/                       # Utilities
│       ├── __init__.py
│       ├── config.py                # Configuration management
│       ├── dpi_scaling.py           # High-DPI support
│       ├── logger.py                # Logging setup
│       └── layout_manager.py        # UI layout persistence
│
├── config/                          # Configuration files
│   ├── default_config.json          # Default settings
│   └── user_config.json             # User settings (gitignored)
│
├── database/                        # Database files (gitignored)
│   └── default.db
│
├── cache/                           # Temporary cache (gitignored)
│   └── waveforms/
│
├── logs/                            # Log files (gitignored)
│
├── tests/                           # Unit and integration tests
│   ├── test_scanner.py
│   ├── test_search.py
│   ├── test_audio.py
│   └── test_database.py
│
├── docs/                            # Documentation
│   ├── AudioLib_Explorer_PRD.md
│   ├── Technical_Architecture.md
│   └── API_Design.md
│
├── requirements.txt                 # Python dependencies
├── setup.py                         # Package setup
├── README.md                        # Project readme
└── LICENSE                          # Software license
```

### 5.2 Module Dependencies

```
┌─────────────┐
│  main.py    │ Entry point
└──────┬──────┘
       │
       ▼
┌────────────────────────────────────────────────┐
│            gui.main_window                     │
│  ┌──────────────────────────────────────────┐ │
│  │  Components: folder_tree, results_list,  │ │
│  │  waveform_viewer, spectrum_analyzer      │ │
│  └──────────────────────────────────────────┘ │
└──────┬─────────────────────────┬───────────────┘
       │                         │
       ▼                         ▼
┌──────────────┐        ┌─────────────────┐
│  i18n        │        │ utils           │
│  (trans.)    │        │ (config, dpi)   │
└──────────────┘        └─────────────────┘
       │
       ▼
┌─────────────────────────────────────────────────┐
│          Business Logic Services                │
│  ┌──────────┐ ┌──────────┐ ┌──────────────┐   │
│  │ scanner  │ │ search   │ │ audio        │   │
│  │ service  │ │ engine   │ │ player       │   │
│  └──────────┘ └──────────┘ └──────────────┘   │
│                                                 │
│  ┌──────────┐ ┌──────────┐ ┌──────────────┐   │
│  │ metadata │ │ waveform │ │ plugins      │   │
│  │ extract  │ │ renderer │ │ manager      │   │
│  └──────────┘ └──────────┘ └──────────────┘   │
└───────────────────┬─────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────┐
│         database.data_provider                  │
│  ┌──────────────────────────────────────────┐  │
│  │  LocalDataProvider / RemoteDataProvider  │  │
│  └──────────────────────────────────────────┘  │
└───────────────────┬─────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────┐
│            database.models (ORM)                │
│  AudioFile, WaveformCache, Playlist, ...        │
└───────────────────┬─────────────────────────────┘
                    │
                    ▼
             ┌──────────────┐
             │ SQLite DB    │
             │ (with FTS5)  │
             └──────────────┘
```

---

## 6. Plugin Architecture

### 6.1 Plugin System Design

**Goals:**
- Enable third-party waveform renderers, spectrum analyzers, and metadata extractors
- Isolate plugin failures from main application
- Simple plugin development with minimal boilerplate

**Plugin Discovery:** Filesystem-based

```
plugins/
├── color_waveform/
│   ├── manifest.json
│   ├── plugin.py
│   └── README.md
└── advanced_spectrum/
    ├── manifest.json
    ├── plugin.py
    └── config.json
```

### 6.2 Plugin Manifest Schema

```json
{
  "$schema": "https://audiolib-explorer.com/schemas/plugin-manifest-v1.json",
  "name": "Color Waveform Renderer",
  "id": "com.audiolib.color-waveform",
  "version": "1.0.0",
  "description": "Renders waveforms with frequency-based coloring",
  "author": "AudioLib Community",
  "license": "MIT",

  "plugin_type": "waveform_renderer",
  "entry_point": "plugin.ColorWaveformRenderer",

  "dependencies": {
    "python": ">=3.11",
    "librosa": ">=0.10.0",
    "matplotlib": ">=3.7.0"
  },

  "configuration_schema": {
    "color_scheme": {
      "type": "string",
      "enum": ["spectrum", "thermal", "grayscale"],
      "default": "spectrum"
    },
    "amplitude_scaling": {
      "type": "string",
      "enum": ["linear", "logarithmic"],
      "default": "logarithmic"
    }
  },

  "permissions": [
    "read_audio_data",
    "render_to_canvas"
  ]
}
```

### 6.3 Plugin Base Classes

```python
# src/plugins/base.py

from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, Dict
import numpy as np
import tkinter as tk

@dataclass
class PluginInfo:
    name: str
    version: str
    description: str
    author: str

class PluginBase(ABC):
    """Base class for all plugins"""

    @abstractmethod
    def get_info(self) -> PluginInfo:
        """Return plugin metadata"""
        pass

    @abstractmethod
    def initialize(self, context: Dict[str, Any]) -> bool:
        """
        Initialize plugin with application context

        Args:
            context: {
                'config': ConfigManager,
                'logger': Logger,
                'data_provider': DataProvider,
                'plugin_dir': str
            }

        Returns:
            True if initialization successful
        """
        pass

    @abstractmethod
    def cleanup(self) -> None:
        """Cleanup resources before unload"""
        pass


class WaveformRendererPlugin(PluginBase):
    """Plugin type: Custom waveform visualization"""

    @abstractmethod
    def render(self,
               audio_data: np.ndarray,
               sample_rate: int,
               canvas: tk.Canvas,
               width: int,
               height: int) -> None:
        """
        Render waveform to tkinter canvas

        Args:
            audio_data: Audio samples (mono or stereo)
            sample_rate: Sample rate in Hz
            canvas: tkinter Canvas widget
            width: Canvas width in pixels
            height: Canvas height in pixels
        """
        pass


class SpectrumAnalyzerPlugin(PluginBase):
    """Plugin type: Spectrum analysis and visualization"""

    @abstractmethod
    def compute_spectrum(self,
                        audio_data: np.ndarray,
                        sample_rate: int,
                        fft_size: int = 2048) -> np.ndarray:
        """
        Compute frequency spectrum

        Args:
            audio_data: Audio samples
            sample_rate: Sample rate in Hz
            fft_size: FFT window size

        Returns:
            Spectrum data (frequency bins x magnitude)
        """
        pass

    @abstractmethod
    def render_spectrum(self,
                       spectrum_data: np.ndarray,
                       canvas: tk.Canvas,
                       width: int,
                       height: int) -> None:
        """Render spectrum to canvas"""
        pass


class MetadataExtractorPlugin(PluginBase):
    """Plugin type: Custom metadata extraction"""

    @abstractmethod
    def extract(self, file_path: str) -> Dict[str, Any]:
        """
        Extract custom metadata from audio file

        Args:
            file_path: Absolute path to audio file

        Returns:
            Dictionary of metadata fields
        """
        pass
```

### 6.4 Example Plugin Implementation

```python
# plugins/color_waveform/plugin.py

import numpy as np
import librosa
import tkinter as tk
from src.plugins.base import WaveformRendererPlugin, PluginInfo

class ColorWaveformRenderer(WaveformRendererPlugin):
    def __init__(self):
        self.config = {}
        self.logger = None

    def get_info(self) -> PluginInfo:
        return PluginInfo(
            name="Color Waveform Renderer",
            version="1.0.0",
            description="Renders waveform with frequency-based coloring",
            author="AudioLib Community"
        )

    def initialize(self, context) -> bool:
        self.config = context.get('config', {})
        self.logger = context.get('logger')
        self.logger.info(f"Initialized {self.get_info().name}")
        return True

    def cleanup(self) -> None:
        self.logger.info(f"Cleaning up {self.get_info().name}")

    def render(self, audio_data, sample_rate, canvas, width, height):
        """Render colorful waveform based on frequency content"""
        canvas.delete('all')  # Clear previous

        # Compute STFT for frequency information
        stft = librosa.stft(audio_data, n_fft=2048, hop_length=512)
        spectral_centroid = librosa.feature.spectral_centroid(S=np.abs(stft), sr=sample_rate)[0]

        # Resample spectral centroid to match waveform resolution
        target_points = min(width, len(audio_data))
        spectral_centroid_resampled = np.interp(
            np.linspace(0, len(spectral_centroid), target_points),
            np.arange(len(spectral_centroid)),
            spectral_centroid
        )

        # Decimate audio for display
        decimation = max(1, len(audio_data) // target_points)
        waveform_decimated = audio_data[::decimation]

        # Map frequency to color (blue=low, red=high)
        colors = self._frequency_to_rgb(spectral_centroid_resampled)

        # Render waveform with colors
        x_scale = width / len(waveform_decimated)
        y_scale = height / 2

        for i, (sample, color) in enumerate(zip(waveform_decimated, colors)):
            x = i * x_scale
            y_top = height/2 - sample * y_scale
            y_bottom = height/2

            canvas.create_line(
                x, y_bottom, x, y_top,
                fill=color,
                width=1
            )

    def _frequency_to_rgb(self, frequencies):
        """Map frequency values to RGB color strings"""
        # Normalize to 0-1 range
        freq_norm = (frequencies - frequencies.min()) / (frequencies.max() - frequencies.min() + 1e-6)

        # Blue (low freq) → Green → Red (high freq)
        colors = []
        for f in freq_norm:
            if f < 0.5:
                # Blue to Green
                r = 0
                g = int(f * 2 * 255)
                b = int((1 - f * 2) * 255)
            else:
                # Green to Red
                r = int((f - 0.5) * 2 * 255)
                g = int((1 - (f - 0.5) * 2) * 255)
                b = 0

            colors.append(f"#{r:02x}{g:02x}{b:02x}")

        return colors
```

### 6.5 Plugin Manager

```python
# src/core/plugins.py (adapted from Video_Explorer)

import importlib.util
import json
from pathlib import Path
from typing import Dict, List, Type
from src.plugins.base import PluginBase
from src.utils.logger import get_logger

logger = get_logger(__name__)

class PluginManager:
    def __init__(self, plugins_dir: str):
        self.plugins_dir = Path(plugins_dir)
        self.loaded_plugins: Dict[str, PluginBase] = {}
        self.plugin_manifests: Dict[str, dict] = {}

    def discover_plugins(self) -> List[str]:
        """Scan plugins directory for valid plugins"""
        discovered = []

        for plugin_dir in self.plugins_dir.iterdir():
            if not plugin_dir.is_dir():
                continue

            manifest_path = plugin_dir / "manifest.json"
            if not manifest_path.exists():
                logger.warning(f"Plugin {plugin_dir.name} missing manifest.json")
                continue

            try:
                with open(manifest_path, 'r', encoding='utf-8') as f:
                    manifest = json.load(f)

                # Validate manifest
                required_fields = ['name', 'id', 'version', 'plugin_type', 'entry_point']
                if not all(field in manifest for field in required_fields):
                    logger.error(f"Plugin {plugin_dir.name} has invalid manifest")
                    continue

                self.plugin_manifests[manifest['id']] = manifest
                discovered.append(manifest['id'])
                logger.info(f"Discovered plugin: {manifest['name']} v{manifest['version']}")

            except Exception as e:
                logger.error(f"Failed to load manifest for {plugin_dir.name}: {e}")

        return discovered

    def load_plugin(self, plugin_id: str, context: dict) -> bool:
        """Load and initialize a plugin"""
        if plugin_id not in self.plugin_manifests:
            logger.error(f"Plugin {plugin_id} not found")
            return False

        if plugin_id in self.loaded_plugins:
            logger.warning(f"Plugin {plugin_id} already loaded")
            return True

        manifest = self.plugin_manifests[plugin_id]

        try:
            # Load plugin module
            plugin_dir = self.plugins_dir / Path(manifest['id'].split('.')[-1])
            plugin_file = plugin_dir / "plugin.py"

            spec = importlib.util.spec_from_file_location(manifest['id'], plugin_file)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)

            # Instantiate plugin class
            entry_point = manifest['entry_point']
            module_name, class_name = entry_point.rsplit('.', 1)
            plugin_class = getattr(module, class_name)
            plugin_instance = plugin_class()

            # Initialize plugin
            if not plugin_instance.initialize(context):
                logger.error(f"Plugin {plugin_id} initialization failed")
                return False

            self.loaded_plugins[plugin_id] = plugin_instance
            logger.info(f"Loaded plugin: {manifest['name']}")
            return True

        except Exception as e:
            logger.error(f"Failed to load plugin {plugin_id}: {e}", exc_info=True)
            return False

    def unload_plugin(self, plugin_id: str) -> bool:
        """Unload a plugin"""
        if plugin_id not in self.loaded_plugins:
            return False

        try:
            plugin = self.loaded_plugins[plugin_id]
            plugin.cleanup()
            del self.loaded_plugins[plugin_id]
            logger.info(f"Unloaded plugin: {plugin_id}")
            return True
        except Exception as e:
            logger.error(f"Failed to unload plugin {plugin_id}: {e}")
            return False

    def get_plugin(self, plugin_id: str) -> PluginBase:
        """Get loaded plugin instance"""
        return self.loaded_plugins.get(plugin_id)

    def get_plugins_by_type(self, plugin_type: str) -> List[PluginBase]:
        """Get all loaded plugins of a specific type"""
        result = []
        for plugin_id, plugin in self.loaded_plugins.items():
            manifest = self.plugin_manifests[plugin_id]
            if manifest['plugin_type'] == plugin_type:
                result.append(plugin)
        return result
```

---

## 7. Multi-Language Framework

### 7.1 Translation System Architecture

**Design Goals:**
- No hardcoded UI text anywhere in codebase
- Runtime language switching without restart
- Support for parameter interpolation and pluralization
- Easy addition of new languages

### 7.2 Translation Manager Implementation

```python
# src/i18n/translations.py

import json
from pathlib import Path
from typing import Dict, Any
from src.utils.logger import get_logger

logger = get_logger(__name__)

class TranslationManager:
    def __init__(self, locales_dir: str = None):
        if locales_dir is None:
            locales_dir = Path(__file__).parent / 'locales'

        self.locales_dir = Path(locales_dir)
        self.current_language = 'en'
        self.translations: Dict[str, Dict[str, Any]] = {}
        self.fallback_language = 'en'

        self._load_all_translations()

    def _load_all_translations(self):
        """Load all translation files from locales directory"""
        for locale_file in self.locales_dir.glob('*.json'):
            lang_code = locale_file.stem

            try:
                with open(locale_file, 'r', encoding='utf-8') as f:
                    self.translations[lang_code] = json.load(f)
                logger.info(f"Loaded translations for language: {lang_code}")
            except Exception as e:
                logger.error(f"Failed to load translations for {lang_code}: {e}")

    def set_language(self, lang_code: str):
        """Switch application language"""
        if lang_code not in self.translations:
            logger.warning(f"Language '{lang_code}' not available, using fallback")
            lang_code = self.fallback_language

        self.current_language = lang_code
        logger.info(f"Language switched to: {lang_code}")

    def get_available_languages(self) -> List[str]:
        """Get list of available language codes"""
        return list(self.translations.keys())

    def t(self, key: str, **kwargs) -> str:
        """
        Translate a key to current language with parameter interpolation

        Args:
            key: Translation key in dot notation (e.g., 'menu.file.open')
            **kwargs: Parameters for string formatting

        Returns:
            Translated string, or key if translation not found

        Example:
            t('status.scanning', current=100, total=500)
            → "Scanning: 100/500 files" (en)
            → "扫描中: 100/500 文件" (cn)
        """
        # Navigate nested dictionary using dot notation
        keys = key.split('.')
        value = self.translations.get(self.current_language, {})

        for k in keys:
            if isinstance(value, dict):
                value = value.get(k)
            else:
                value = None
                break

        # Fallback to English if not found
        if value is None:
            value = self._get_fallback(key)

        # Fallback to key itself if still not found
        if value is None:
            logger.warning(f"Translation key not found: {key}")
            return key

        # Apply string formatting if parameters provided
        if kwargs:
            try:
                value = value.format(**kwargs)
            except KeyError as e:
                logger.error(f"Missing parameter for translation key {key}: {e}")

        return value

    def _get_fallback(self, key: str) -> str:
        """Get translation from fallback language"""
        keys = key.split('.')
        value = self.translations.get(self.fallback_language, {})

        for k in keys:
            if isinstance(value, dict):
                value = value.get(k)
            else:
                return None

        return value


# Global instance
_translation_manager = None

def init_translation_manager(locales_dir: str = None):
    """Initialize global translation manager"""
    global _translation_manager
    _translation_manager = TranslationManager(locales_dir)

def get_translation_manager() -> TranslationManager:
    """Get global translation manager instance"""
    global _translation_manager
    if _translation_manager is None:
        init_translation_manager()
    return _translation_manager

def t(key: str, **kwargs) -> str:
    """Shortcut function for translations"""
    return get_translation_manager().t(key, **kwargs)

def set_language(lang_code: str):
    """Shortcut to change language"""
    get_translation_manager().set_language(lang_code)
```

### 7.3 Translation Files

**English (src/i18n/locales/en.json):**

```json
{
  "app": {
    "title": "AudioLib Explorer",
    "version": "Version {version}"
  },
  "menu": {
    "file": {
      "title": "File",
      "new_database": "New Database...",
      "open_database": "Open Database...",
      "close_database": "Close Database",
      "scan_directory": "Scan Directory...",
      "exit": "Exit"
    },
    "edit": {
      "title": "Edit",
      "preferences": "Preferences...",
      "edit_tags": "Edit Tags...",
      "edit_notes": "Edit Notes..."
    },
    "view": {
      "title": "View",
      "folder_tree": "Folder Tree",
      "waveform": "Waveform Viewer",
      "spectrum": "Spectrum Analyzer",
      "reset_layout": "Reset Layout"
    },
    "tools": {
      "title": "Tools",
      "optimize_database": "Optimize Database",
      "check_integrity": "Check Database Integrity",
      "export_list": "Export File List..."
    },
    "help": {
      "title": "Help",
      "keyboard_shortcuts": "Keyboard Shortcuts",
      "about": "About AudioLib Explorer"
    }
  },
  "toolbar": {
    "scan": "Scan",
    "refresh": "Refresh",
    "search_placeholder": "Search audio files..."
  },
  "columns": {
    "filename": "Filename",
    "duration": "Duration",
    "sample_rate": "Sample Rate",
    "bit_depth": "Bit Depth",
    "channels": "Channels",
    "file_size": "Size",
    "rating": "Rating",
    "tags": "Tags",
    "created_date": "Created",
    "modified_date": "Modified"
  },
  "filters": {
    "title": "Filters",
    "sample_rate": "Sample Rate",
    "bit_depth": "Bit Depth",
    "channels": "Channels",
    "duration": "Duration",
    "duration_min": "Min (seconds)",
    "duration_max": "Max (seconds)",
    "rating": "Rating",
    "favorites_only": "Favorites Only",
    "clear_filters": "Clear All Filters"
  },
  "player": {
    "play": "Play",
    "pause": "Pause",
    "stop": "Stop",
    "volume": "Volume",
    "position": "{current} / {total}"
  },
  "status": {
    "ready": "Ready",
    "scanning": "Scanning: {current}/{total} files",
    "scan_complete": "Scan complete: {added} added, {updated} updated, {failed} failed",
    "searching": "Searching...",
    "search_results": "{count} results found",
    "loading": "Loading...",
    "files_in_database": "{count} files in database",
    "database": "Database: {name}"
  },
  "dialogs": {
    "scan": {
      "title": "Scan Directory for Audio Files",
      "directory": "Directory:",
      "browse": "Browse...",
      "recursive": "Include subdirectories",
      "pattern": "File pattern:",
      "add_to_database": "Add to current database",
      "scan": "Scan",
      "cancel": "Cancel"
    },
    "preferences": {
      "title": "Preferences",
      "general": "General",
      "database": "Database",
      "audio": "Audio",
      "performance": "Performance",
      "language": "Language:",
      "theme": "Theme:",
      "ok": "OK",
      "cancel": "Cancel",
      "apply": "Apply"
    },
    "about": {
      "title": "About AudioLib Explorer",
      "version": "Version {version}",
      "copyright": "Copyright © 2025 AudioLib Team",
      "description": "High-performance audio library management for sound designers",
      "close": "Close"
    }
  },
  "messages": {
    "error": {
      "database_open_failed": "Failed to open database: {error}",
      "scan_failed": "Scan failed: {error}",
      "file_not_found": "File not found: {path}",
      "invalid_audio_file": "Invalid audio file: {path}"
    },
    "warning": {
      "database_not_open": "No database is currently open",
      "no_files_selected": "No files selected"
    },
    "info": {
      "database_optimized": "Database optimized successfully",
      "tags_saved": "Tags saved"
    },
    "confirm": {
      "delete_files": "Delete {count} file(s) from database?"
    }
  }
}
```

**Simplified Chinese (src/i18n/locales/cn.json):**

```json
{
  "app": {
    "title": "音频库浏览器",
    "version": "版本 {version}"
  },
  "menu": {
    "file": {
      "title": "文件",
      "new_database": "新建数据库...",
      "open_database": "打开数据库...",
      "close_database": "关闭数据库",
      "scan_directory": "扫描目录...",
      "exit": "退出"
    },
    "edit": {
      "title": "编辑",
      "preferences": "首选项...",
      "edit_tags": "编辑标签...",
      "edit_notes": "编辑备注..."
    },
    "view": {
      "title": "查看",
      "folder_tree": "文件夹树",
      "waveform": "波形查看器",
      "spectrum": "频谱分析器",
      "reset_layout": "重置布局"
    },
    "tools": {
      "title": "工具",
      "optimize_database": "优化数据库",
      "check_integrity": "检查数据库完整性",
      "export_list": "导出文件列表..."
    },
    "help": {
      "title": "帮助",
      "keyboard_shortcuts": "键盘快捷键",
      "about": "关于音频库浏览器"
    }
  },
  "toolbar": {
    "scan": "扫描",
    "refresh": "刷新",
    "search_placeholder": "搜索音频文件..."
  },
  "columns": {
    "filename": "文件名",
    "duration": "时长",
    "sample_rate": "采样率",
    "bit_depth": "位深度",
    "channels": "声道",
    "file_size": "大小",
    "rating": "评分",
    "tags": "标签",
    "created_date": "创建日期",
    "modified_date": "修改日期"
  },
  "filters": {
    "title": "筛选器",
    "sample_rate": "采样率",
    "bit_depth": "位深度",
    "channels": "声道数",
    "duration": "时长",
    "duration_min": "最小值(秒)",
    "duration_max": "最大值(秒)",
    "rating": "评分",
    "favorites_only": "仅收藏",
    "clear_filters": "清除所有筛选"
  },
  "player": {
    "play": "播放",
    "pause": "暂停",
    "stop": "停止",
    "volume": "音量",
    "position": "{current} / {total}"
  },
  "status": {
    "ready": "就绪",
    "scanning": "扫描中: {current}/{total} 文件",
    "scan_complete": "扫描完成: {added} 已添加, {updated} 已更新, {failed} 失败",
    "searching": "搜索中...",
    "search_results": "找到 {count} 个结果",
    "loading": "加载中...",
    "files_in_database": "数据库中有 {count} 个文件",
    "database": "数据库: {name}"
  },
  "dialogs": {
    "scan": {
      "title": "扫描目录查找音频文件",
      "directory": "目录:",
      "browse": "浏览...",
      "recursive": "包含子目录",
      "pattern": "文件模式:",
      "add_to_database": "添加到当前数据库",
      "scan": "扫描",
      "cancel": "取消"
    },
    "preferences": {
      "title": "首选项",
      "general": "常规",
      "database": "数据库",
      "audio": "音频",
      "performance": "性能",
      "language": "语言:",
      "theme": "主题:",
      "ok": "确定",
      "cancel": "取消",
      "apply": "应用"
    },
    "about": {
      "title": "关于音频库浏览器",
      "version": "版本 {version}",
      "copyright": "版权所有 © 2025 AudioLib 团队",
      "description": "为声音设计师打造的高性能音频库管理工具",
      "close": "关闭"
    }
  },
  "messages": {
    "error": {
      "database_open_failed": "打开数据库失败: {error}",
      "scan_failed": "扫描失败: {error}",
      "file_not_found": "文件未找到: {path}",
      "invalid_audio_file": "无效的音频文件: {path}"
    },
    "warning": {
      "database_not_open": "当前没有打开的数据库",
      "no_files_selected": "未选择任何文件"
    },
    "info": {
      "database_optimized": "数据库优化成功",
      "tags_saved": "标签已保存"
    },
    "confirm": {
      "delete_files": "确定要从数据库删除 {count} 个文件吗?"
    }
  }
}
```

### 7.4 Usage in GUI Code

```python
# src/gui/main_window.py

import tkinter as tk
from tkinter import ttk
from src.i18n.translations import t, set_language

class MainWindow(tk.Tk):
    def __init__(self):
        super().__init__()

        # Set window title (translated)
        self.title(t('app.title'))

        # Create menu bar
        self.create_menu()

        # Create toolbar
        self.create_toolbar()

        # Create status bar
        self.status_var = tk.StringVar(value=t('status.ready'))
        self.status_bar = ttk.Label(self, textvariable=self.status_var, relief=tk.SUNKEN)
        self.status_bar.pack(side=tk.BOTTOM, fill=tk.X)

    def create_menu(self):
        menubar = tk.Menu(self)

        # File menu
        file_menu = tk.Menu(menubar, tearoff=0)
        file_menu.add_command(
            label=t('menu.file.scan_directory'),
            command=self.scan_directory
        )
        file_menu.add_separator()
        file_menu.add_command(
            label=t('menu.file.exit'),
            command=self.quit
        )
        menubar.add_cascade(
            label=t('menu.file.title'),
            menu=file_menu
        )

        # View menu
        view_menu = tk.Menu(menubar, tearoff=0)
        view_menu.add_checkbutton(
            label=t('menu.view.waveform'),
            command=self.toggle_waveform
        )
        view_menu.add_checkbutton(
            label=t('menu.view.spectrum'),
            command=self.toggle_spectrum
        )
        menubar.add_cascade(
            label=t('menu.view.title'),
            menu=view_menu
        )

        self.config(menu=menubar)

    def create_toolbar(self):
        toolbar = ttk.Frame(self)

        # Scan button
        scan_btn = ttk.Button(
            toolbar,
            text=t('toolbar.scan'),
            command=self.scan_directory
        )
        scan_btn.pack(side=tk.LEFT, padx=2, pady=2)

        # Search box
        search_var = tk.StringVar()
        search_entry = ttk.Entry(
            toolbar,
            textvariable=search_var,
            width=40
        )
        search_entry.pack(side=tk.LEFT, padx=10, pady=2, fill=tk.X, expand=True)

        # Placeholder text
        search_entry.insert(0, t('toolbar.search_placeholder'))
        search_entry.bind('<FocusIn>', self.clear_placeholder)

        toolbar.pack(side=tk.TOP, fill=tk.X)

    def refresh_ui(self):
        """Refresh all UI elements after language change"""
        self.title(t('app.title'))
        self.status_var.set(t('status.ready'))

        # Recreate menu and toolbar with new translations
        self.config(menu=tk.Menu(self))
        self.create_menu()

        # Note: In real implementation, would need to update all child widgets

    def change_language(self, lang_code: str):
        """Change UI language"""
        set_language(lang_code)
        self.refresh_ui()
```

### 7.5 Chinese Text Segmentation (中文分词)

#### 7.5.1 Problem Statement

**Issue:** SQLite FTS5's `unicode61` tokenizer does not support Chinese word segmentation.

**Example:**
- Search query: "声音设计" (sound design)
- Expected: Match documents containing "声音" OR "设计"
- Actual (without segmentation): Only exact match "声音设计"

**Impact:** Poor search experience for Chinese users.

#### 7.5.2 Solution: Jieba Integration

**Library:** jieba (结巴中文分词)
- Most popular Chinese text segmentation library
- Fast and accurate
- Supports custom dictionaries

**Installation:**
```bash
pip install jieba==0.42.1
```

#### 7.5.3 Custom FTS5 Tokenizer

**Implementation:**

```python
# src/search/chinese_tokenizer.py

import jieba
import re
from typing import Iterator, Tuple

class ChineseTokenizer:
    """Custom tokenizer for Chinese text segmentation"""

    def __init__(self):
        # Initialize jieba
        jieba.initialize()

        # Add custom audio-related dictionary
        self._add_audio_vocabulary()

    def _add_audio_vocabulary(self):
        """Add audio-specific terms to jieba dictionary"""
        audio_terms = [
            '声音设计', '音效', '采样率', '位深度', '波形', '频谱',
            '爆炸音', '脚步声', '环境音', '武器音', '交通音',
            # ... more audio terms
        ]

        for term in audio_terms:
            jieba.add_word(term, freq=10000)  # High frequency

    def contains_chinese(self, text: str) -> bool:
        """Check if text contains Chinese characters"""
        return bool(re.search(r'[\u4e00-\u9fff]', text))

    def tokenize(self, text: str) -> Iterator[str]:
        """
        Tokenize text with Chinese segmentation support

        Args:
            text: Input text (mixed Chinese and English)

        Yields:
            Tokens (words)
        """
        if not text:
            return

        # If text contains Chinese, use jieba
        if self.contains_chinese(text):
            # jieba.cut returns generator of tokens
            for token in jieba.cut(text, cut_all=False):
                token = token.strip()
                if token:
                    yield token
        else:
            # For English text, use simple whitespace split
            for token in text.split():
                token = token.strip()
                if token:
                    yield token

# Global instance
_tokenizer = None

def get_chinese_tokenizer() -> ChineseTokenizer:
    """Get global Chinese tokenizer instance"""
    global _tokenizer
    if _tokenizer is None:
        _tokenizer = ChineseTokenizer()
    return _tokenizer
```

#### 7.5.4 Pre-Tokenization Strategy

**Approach:** Pre-tokenize Chinese text before inserting into FTS5 index.

**Why:** SQLite FTS5 doesn't support custom tokenizers easily. Pre-tokenization is simpler and more reliable.

**Implementation:**

```python
# src/database/models.py (modify AudioFile model)

from src.search.chinese_tokenizer import get_chinese_tokenizer

class AudioFile(Base):
    __tablename__ = 'audio_files'

    # ... existing fields

    # Add tokenized fields for FTS5
    filename_tokenized = Column(Text)  # Space-separated tokens
    user_tags_tokenized = Column(Text)
    user_notes_tokenized = Column(Text)

    def _tokenize_text(self, text: str) -> str:
        """Tokenize text for FTS5 indexing"""
        if not text:
            return ""

        tokenizer = get_chinese_tokenizer()
        tokens = list(tokenizer.tokenize(text))
        return " ".join(tokens)

    def update_tokenized_fields(self):
        """Update all tokenized fields"""
        self.filename_tokenized = self._tokenize_text(self.filename)
        self.user_tags_tokenized = self._tokenize_text(self.user_tags)
        self.user_notes_tokenized = self._tokenize_text(self.user_notes)
```

**Modified FTS5 Table:**

```sql
-- Use tokenized fields instead of original fields
CREATE VIRTUAL TABLE audio_search USING fts5(
    filename_tokenized,
    user_tags_tokenized,
    user_notes_tokenized,
    bext_description,  -- English only, no tokenization needed
    file_path,
    content='audio_files',
    content_rowid='id',
    tokenize='unicode61'  -- Standard tokenizer, but text is pre-tokenized
);

-- Triggers to update FTS index
CREATE TRIGGER audio_search_ai AFTER INSERT ON audio_files BEGIN
  -- Update tokenized fields
  UPDATE audio_files
  SET filename_tokenized = (SELECT tokenize_chinese(NEW.filename)),
      user_tags_tokenized = (SELECT tokenize_chinese(NEW.user_tags)),
      user_notes_tokenized = (SELECT tokenize_chinese(NEW.user_notes))
  WHERE id = NEW.id;

  -- Insert into FTS index
  INSERT INTO audio_search(rowid, filename_tokenized, user_tags_tokenized, ...)
  VALUES (NEW.id, NEW.filename_tokenized, NEW.user_tags_tokenized, ...);
END;
```

#### 7.5.5 Search Query Tokenization

**User Input Processing:**

```python
# src/search/search_engine.py

from src.search.chinese_tokenizer import get_chinese_tokenizer

class SearchEngine:
    def search(self, query: str, filters: dict = None):
        """Search with Chinese tokenization support"""

        # Tokenize search query
        tokenizer = get_chinese_tokenizer()
        query_tokens = list(tokenizer.tokenize(query))
        tokenized_query = " ".join(query_tokens)

        # Use tokenized query for FTS5 search
        sql = """
            SELECT audio_files.*
            FROM audio_files
            JOIN audio_search ON audio_files.id = audio_search.rowid
            WHERE audio_search MATCH :query
            ORDER BY rank
            LIMIT :limit
        """

        results = self.session.execute(sql, {
            'query': tokenized_query,
            'limit': 1000
        })

        return results.fetchall()
```

**Example:**

```python
# User searches: "爆炸声音"
# Tokenized: "爆炸 声音"
# FTS5 matches: Documents containing "爆炸" OR "声音"

engine = SearchEngine(session)
results = engine.search("爆炸声音")
# Returns files with tags like "爆炸", "声音", "爆炸音效", etc.
```

#### 7.5.6 Performance Considerations

**Tokenization Overhead:**
- **On Insert:** ~0.5ms per file (negligible)
- **On Search:** ~1ms for query tokenization (negligible)

**Storage Overhead:**
- Tokenized fields add ~20% to database size
- Acceptable trade-off for Chinese search functionality

**Optimization:**
- Tokenization done once during insert/update
- Search queries cached after tokenization

#### 7.5.7 Alternative: Full Custom Tokenizer (Advanced)

**For Phase 2 or if performance critical:**

Implement native SQLite FTS5 custom tokenizer in C:

```c
// Requires compiling SQLite extension
// Not recommended for Phase 1 due to complexity

#include <sqlite3ext.h>
#include "jieba.h"  // C wrapper for jieba

static int jieba_tokenizer_create(/* ... */) {
    // Initialize jieba tokenizer
}

static int jieba_tokenizer_tokenize(/* ... */) {
    // Tokenize using jieba
}

// Register tokenizer with SQLite
sqlite3_create_module(db, "jieba", &jieba_module, NULL);
```

**Usage:**
```sql
CREATE VIRTUAL TABLE audio_search USING fts5(
    filename,
    tokenize='jieba'  -- Custom tokenizer
);
```

**Pros:** No pre-tokenization needed, cleaner architecture
**Cons:** Complex build process, platform-specific compilation

**Recommendation:** Use pre-tokenization approach (7.5.4) for Phase 1.

---

## 8. Error Handling and Logging Strategy

### 8.1 Error Classification

AudioLib Explorer uses a hierarchical error classification system to ensure appropriate handling and user feedback.

#### 8.1.1 Error Categories

```python
# src/utils/exceptions.py

class AudioLibException(Exception):
    """Base exception for all AudioLib Explorer errors"""
    def __init__(self, message: str, details: dict = None):
        super().__init__(message)
        self.message = message
        self.details = details or {}
        self.timestamp = datetime.now()

class UserRecoverableError(AudioLibException):
    """Errors that users can recover from with guidance"""
    pass

class SystemError(AudioLibException):
    """Errors requiring system-level intervention"""
    pass

class DataIntegrityError(AudioLibException):
    """Database or data corruption errors"""
    pass

# Specific error types

class AudioFileError(UserRecoverableError):
    """Audio file processing errors"""
    pass

class UnsupportedFormatError(AudioFileError):
    """File format not supported"""
    pass

class CorruptedFileError(AudioFileError):
    """File is corrupted or unreadable"""
    pass

class DatabaseError(SystemError):
    """Database operation failures"""
    pass

class SearchEngineError(SystemError):
    """FTS5 search engine errors"""
    pass

class ValidationError(UserRecoverableError):
    """Input validation failures"""
    pass

class PluginError(UserRecoverableError):
    """Plugin loading or execution errors"""
    pass
```

#### 8.1.2 Error Handling Patterns

**Pattern 1: Graceful Degradation**
```python
# src/scanner/metadata_extractor.py

def extract_metadata(file_path: str) -> AudioMetadata:
    """Extract audio metadata with graceful degradation"""
    try:
        metadata = extract_full_metadata(file_path)
        return metadata
    except CorruptedFileError as e:
        logger.warning(f"Corrupted file, extracting basic metadata: {file_path}", exc_info=True)
        # Fall back to basic metadata extraction
        return extract_basic_metadata(file_path)
    except UnsupportedFormatError as e:
        logger.error(f"Unsupported format: {file_path}", exc_info=True)
        raise  # Re-raise to skip this file
    except Exception as e:
        logger.error(f"Unexpected error extracting metadata: {file_path}", exc_info=True)
        # Record failure, continue with next file
        record_failed_file(file_path, str(e))
        return None
```

**Pattern 2: Retry with Backoff**
```python
# src/utils/retry.py

import time
from functools import wraps

def retry_on_error(max_attempts=3, backoff_seconds=1, exceptions=(Exception,)):
    """Decorator for retrying operations with exponential backoff"""
    def decorator(func):
        @wraps(func)
        def wrapper(*args, **kwargs):
            for attempt in range(max_attempts):
                try:
                    return func(*args, **kwargs)
                except exceptions as e:
                    if attempt == max_attempts - 1:
                        logger.error(f"Failed after {max_attempts} attempts: {func.__name__}", exc_info=True)
                        raise
                    wait_time = backoff_seconds * (2 ** attempt)
                    logger.warning(f"Attempt {attempt + 1} failed, retrying in {wait_time}s: {func.__name__}")
                    time.sleep(wait_time)
        return wrapper
    return decorator

# Usage
@retry_on_error(max_attempts=3, exceptions=(DatabaseError,))
def save_to_database(data):
    session.add(data)
    session.commit()
```

**Pattern 3: User-Friendly Error Messages**
```python
# src/ui/error_dialog.py

class ErrorPresenter:
    """Convert technical errors to user-friendly messages"""

    ERROR_MESSAGES = {
        UnsupportedFormatError: {
            'en': "This file format is not supported. AudioLib Explorer only supports WAV files.",
            'cn': "不支持此文件格式。AudioLib Explorer 仅支持 WAV 文件。"
        },
        DatabaseError: {
            'en': "Database error occurred. Please restart the application.",
            'cn': "数据库错误。请重启应用程序。"
        },
        ValidationError: {
            'en': "Invalid input: {details}",
            'cn': "输入无效：{details}"
        }
    }

    @staticmethod
    def present_error(error: AudioLibException, language='en'):
        """Present error to user in appropriate language"""
        error_type = type(error)
        template = ErrorPresenter.ERROR_MESSAGES.get(error_type, {}).get(language)

        if template:
            message = template.format(details=error.details.get('message', ''))
        else:
            message = str(error)

        logger.error(f"Presenting error to user: {message}", exc_info=True)
        return message
```

### 8.2 Logging Configuration

#### 8.2.1 Logging Architecture

```python
# src/utils/logging_config.py

import logging
import logging.handlers
from pathlib import Path
import json

def setup_logging(log_dir='logs', log_level='INFO'):
    """Configure application-wide logging"""

    # Create logs directory
    log_path = Path(log_dir)
    log_path.mkdir(exist_ok=True)

    # Root logger configuration
    root_logger = logging.getLogger('audiolib')
    root_logger.setLevel(getattr(logging, log_level))

    # Console handler (INFO and above)
    console_handler = logging.StreamHandler()
    console_handler.setLevel(logging.INFO)
    console_formatter = logging.Formatter(
        '%(asctime)s - %(name)s - %(levelname)s - %(message)s',
        datefmt='%H:%M:%S'
    )
    console_handler.setFormatter(console_formatter)

    # File handler with rotation (all levels)
    file_handler = logging.handlers.RotatingFileHandler(
        log_path / 'audiolib.log',
        maxBytes=10 * 1024 * 1024,  # 10MB
        backupCount=5,
        encoding='utf-8'
    )
    file_handler.setLevel(logging.DEBUG)
    file_formatter = logging.Formatter(
        '%(asctime)s - %(name)s - %(levelname)s - [%(filename)s:%(lineno)d] - %(message)s',
        datefmt='%Y-%m-%d %H:%M:%S'
    )
    file_handler.setFormatter(file_formatter)

    # Error file handler (ERROR and CRITICAL only)
    error_handler = logging.handlers.RotatingFileHandler(
        log_path / 'errors.log',
        maxBytes=10 * 1024 * 1024,  # 10MB
        backupCount=3,
        encoding='utf-8'
    )
    error_handler.setLevel(logging.ERROR)
    error_handler.setFormatter(file_formatter)

    # Add handlers
    root_logger.addHandler(console_handler)
    root_logger.addHandler(file_handler)
    root_logger.addHandler(error_handler)

    return root_logger
```

#### 8.2.2 Logging Levels and Usage

| Level | Usage | Examples |
|-------|-------|----------|
| **DEBUG** | Development diagnostics, detailed state information | SQL queries, cache hits/misses, plugin loading details |
| **INFO** | General application events | Scan started, search completed, file added to library |
| **WARNING** | Unexpected but recoverable situations | Corrupted metadata, missing optional fields, deprecated API usage |
| **ERROR** | Errors that prevent specific operations | File read failure, database query error, plugin crash |
| **CRITICAL** | Errors that may cause application shutdown | Database corruption, out of memory, fatal configuration error |

**Usage Examples:**

```python
# src/scanner/scanner_service.py

logger = logging.getLogger('audiolib.scanner')

class ScannerService:
    def scan_directory(self, path: str):
        logger.info(f"Starting directory scan: {path}")

        try:
            files = self.find_audio_files(path)
            logger.debug(f"Found {len(files)} audio files")

            for file in files:
                try:
                    metadata = self.extract_metadata(file)
                    self.save_to_database(metadata)
                    logger.debug(f"Processed: {file}")

                except CorruptedFileError as e:
                    logger.warning(f"Skipping corrupted file: {file}", exc_info=True)
                    continue

                except Exception as e:
                    logger.error(f"Failed to process file: {file}", exc_info=True)
                    self.failed_files.append((file, str(e)))

            logger.info(f"Scan completed: {len(files)} files processed, {len(self.failed_files)} failed")

        except Exception as e:
            logger.critical(f"Scan failed catastrophically: {path}", exc_info=True)
            raise
```

#### 8.2.3 Structured Logging for Search Performance

```python
# src/search/search_engine.py

import logging
import json

logger = logging.getLogger('audiolib.search')

def log_search_performance(query: str, result_count: int, latency_ms: float, filters: dict = None):
    """Log search performance metrics in structured format"""
    log_data = {
        'event': 'search_completed',
        'query': query,
        'result_count': result_count,
        'latency_ms': round(latency_ms, 2),
        'filters': filters or {},
        'timestamp': datetime.now().isoformat()
    }

    # Log as JSON for easy parsing
    logger.info(f"SEARCH_METRICS: {json.dumps(log_data)}")

    # Warn if performance target exceeded
    if latency_ms > 500:
        logger.warning(f"Search exceeded performance target: {latency_ms:.1f}ms > 500ms")
```

#### 8.2.4 Log Rotation and Management

**Configuration:**
- Main log (`audiolib.log`): 10MB per file, 5 backups (50MB total)
- Error log (`errors.log`): 10MB per file, 3 backups (30MB total)
- Performance log (`performance.log`): 5MB per file, 3 backups (15MB total)

**Cleanup Policy:**
- Automatically rotate logs when size limit reached
- Keep last 7 days of logs
- Compress old logs (gzip) to save space
- Provide UI option to "Clear Old Logs" (>30 days)

### 8.3 Error Recovery Strategies

#### 8.3.1 Database Recovery

```python
# src/database/recovery.py

class DatabaseRecovery:
    """Handle database corruption and recovery"""

    @staticmethod
    def verify_database_integrity(db_path: str) -> bool:
        """Check database integrity using SQLite's PRAGMA integrity_check"""
        try:
            engine = create_engine(f"sqlite:///{db_path}")
            with engine.connect() as conn:
                result = conn.execute("PRAGMA integrity_check").fetchone()
                return result[0] == 'ok'
        except Exception as e:
            logger.error(f"Database integrity check failed: {db_path}", exc_info=True)
            return False

    @staticmethod
    def recover_from_backup(db_path: str, backup_path: str):
        """Restore database from backup"""
        logger.warning(f"Restoring database from backup: {backup_path}")
        shutil.copy2(backup_path, db_path)
        logger.info("Database restored from backup")

    @staticmethod
    def create_backup(db_path: str) -> str:
        """Create database backup"""
        timestamp = datetime.now().strftime('%Y%m%d_%H%M%S')
        backup_path = f"{db_path}.backup_{timestamp}"
        shutil.copy2(db_path, backup_path)
        logger.info(f"Database backup created: {backup_path}")
        return backup_path
```

#### 8.3.2 Graceful Shutdown on Critical Errors

```python
# src/app.py

import sys
import atexit

def register_error_handlers():
    """Register global error handlers"""

    def handle_uncaught_exception(exc_type, exc_value, exc_traceback):
        """Handle uncaught exceptions before crash"""
        if issubclass(exc_type, KeyboardInterrupt):
            sys.__excepthook__(exc_type, exc_value, exc_traceback)
            return

        logger.critical("Uncaught exception", exc_info=(exc_type, exc_value, exc_traceback))

        # Attempt graceful shutdown
        try:
            cleanup_resources()
        except:
            pass

        # Show error dialog to user
        show_crash_dialog(exc_value)

    sys.excepthook = handle_uncaught_exception

    def cleanup_on_exit():
        """Cleanup resources on exit"""
        logger.info("Application shutting down")
        try:
            close_database_connections()
            save_user_preferences()
        except Exception as e:
            logger.error("Error during cleanup", exc_info=True)

    atexit.register(cleanup_on_exit)
```

### 8.4 User Feedback Guidelines

**Error Dialog Design:**

```python
# src/ui/error_dialog.py

def show_error_dialog(error: AudioLibException, parent_window):
    """Display user-friendly error dialog"""

    dialog = ErrorDialog(parent_window)
    dialog.set_title(_("Error Occurred"))  # i18n

    # User-friendly message
    dialog.set_message(ErrorPresenter.present_error(error))

    # Details (expandable)
    dialog.set_details(f"""
    Error Type: {type(error).__name__}
    Time: {error.timestamp}
    Details: {json.dumps(error.details, indent=2)}
    """)

    # Actions
    if isinstance(error, UserRecoverableError):
        dialog.add_button(_("Retry"), callback=retry_action)
        dialog.add_button(_("Skip"), callback=skip_action)
    else:
        dialog.add_button(_("Close"), callback=close_action)
        dialog.add_button(_("Report Bug"), callback=open_bug_report)

    dialog.show()
```

**Error Reporting:**
- Critical errors: Show dialog with "Report Bug" option
- Warnings: Show non-intrusive notification (toast/status bar)
- Info: Log only, no user interruption

---

## 9. Performance Optimization

### 9.1 Database Optimization Strategies

**SQLite Configuration:**

```python
# src/database/db_manager.py

from sqlalchemy import create_engine, event
from sqlalchemy.engine import Engine

def create_optimized_engine(db_path: str):
    """Create SQLite engine with performance optimizations"""
    engine = create_engine(
        f"sqlite:///{db_path}",
        echo=False,  # Disable SQL logging in production
        pool_pre_ping=True,  # Verify connections before use
        connect_args={
            'check_same_thread': False,  # Allow multi-threaded access
            'timeout': 30  # 30-second lock timeout
        }
    )

    # Apply PRAGMA optimizations
    @event.listens_for(Engine, "connect")
    def set_sqlite_pragma(dbapi_conn, connection_record):
        cursor = dbapi_conn.cursor()

        # Write-Ahead Logging for concurrent reads during writes
        cursor.execute("PRAGMA journal_mode=WAL")

        # Synchronous mode (faster but less safe)
        cursor.execute("PRAGMA synchronous=NORMAL")

        # Cache size (10MB negative value = kibibytes)
        cursor.execute("PRAGMA cache_size=-10000")

        # Memory-mapped I/O (faster file reads)
        cursor.execute("PRAGMA mmap_size=268435456")  # 256MB

        # Temp store in memory
        cursor.execute("PRAGMA temp_store=MEMORY")

        cursor.close()

    return engine
```

**Indexing Strategy:**

```sql
-- Covering index for common search query
CREATE INDEX idx_search_covering
ON audio_files(sample_rate, bit_depth, channels, duration, is_favorite)
INCLUDE (filename, file_path, file_size);

-- This allows queries like:
-- SELECT filename, file_path FROM audio_files
-- WHERE sample_rate = 48000 AND is_favorite = 1
-- to be satisfied entirely from the index (no table lookup)
```

**Query Optimization:**

```python
# Bad: N+1 query problem
def get_audio_files_with_tags(session):
    files = session.query(AudioFile).limit(100).all()
    for file in files:
        tags = json.loads(file.user_tags)  # Separate JSON parse per file
        print(f"{file.filename}: {tags}")

# Good: Single query with eager loading
from sqlalchemy.orm import joinedload

def get_audio_files_optimized(session):
    files = session.query(AudioFile).limit(100).all()
    # Parse JSON once for all files
    for file in files:
        if file.user_tags:
            tags = json.loads(file.user_tags)
        else:
            tags = []
        print(f"{file.filename}: {tags}")
```

### 9.2 Search Performance Optimization

**Query Result Caching:**

```python
# src/search/search_engine.py

from functools import lru_cache
import time

class SearchEngine:
    def __init__(self, data_provider):
        self.data_provider = data_provider
        self.cache = {}
        self.cache_ttl = 300  # 5 minutes

    def search(self, query: str, filters: dict = None):
        """Search with caching"""
        # Create cache key
        cache_key = self._make_cache_key(query, filters)

        # Check cache
        if cache_key in self.cache:
            cached_result, timestamp = self.cache[cache_key]
            if time.time() - timestamp < self.cache_ttl:
                logger.debug(f"Cache hit for query: {query}")
                return cached_result

        # Execute search
        results = self._execute_search(query, filters)

        # Cache results
        self.cache[cache_key] = (results, time.time())

        # Prune old cache entries
        self._prune_cache()

        return results

    def _make_cache_key(self, query, filters):
        """Create deterministic cache key"""
        import hashlib
        key_str = f"{query}|{json.dumps(filters, sort_keys=True)}"
        return hashlib.md5(key_str.encode()).hexdigest()

    def _prune_cache(self):
        """Remove expired cache entries"""
        current_time = time.time()
        expired = [
            k for k, (_, timestamp) in self.cache.items()
            if current_time - timestamp > self.cache_ttl
        ]
        for k in expired:
            del self.cache[k]
```

**Search Debouncing:**

```python
# src/gui/components/search_box.py

class SearchBox(ttk.Entry):
    def __init__(self, parent, search_callback, debounce_ms=300):
        super().__init__(parent)

        self.search_callback = search_callback
        self.debounce_ms = debounce_ms
        self.after_id = None

        # Bind key release event
        self.bind('<KeyRelease>', self._on_key_release)

    def _on_key_release(self, event):
        """Debounce search execution"""
        # Cancel previous scheduled search
        if self.after_id:
            self.after_cancel(self.after_id)

        # Schedule new search after delay
        self.after_id = self.after(self.debounce_ms, self._execute_search)

    def _execute_search(self):
        """Execute search callback"""
        query = self.get()
        if query:
            self.search_callback(query)
        self.after_id = None
```

### 9.3 Waveform Rendering Optimization

**Multi-Level Caching Strategy:**

```python
# src/audio/waveform_renderer.py

import numpy as np
import soundfile as sf
import pickle
from src.database.models import WaveformCache

class WaveformRenderer:
    def __init__(self, db_session):
        self.session = db_session

        # Zoom level configurations
        self.zoom_configs = {
            'overview': {
                'samples_per_pixel': 1000,  # 1000:1 decimation
                'description': 'Full file overview'
            },
            'minute': {
                'samples_per_pixel': 100,  # 100:1 decimation
                'description': 'Minute-level detail'
            },
            'second': {
                'samples_per_pixel': 10,  # 10:1 decimation
                'description': 'Second-level detail'
            }
        }

    def get_waveform(self, audio_file_id: int, zoom_level: str = 'overview'):
        """Get waveform data (cached or generated)"""
        # Check database cache first
        cached = self.session.query(WaveformCache).filter_by(
            audio_file_id=audio_file_id,
            zoom_level=zoom_level
        ).first()

        if cached:
            logger.debug(f"Waveform cache hit: file={audio_file_id}, zoom={zoom_level}")
            return pickle.loads(cached.waveform_data)

        # Generate and cache
        logger.info(f"Generating waveform: file={audio_file_id}, zoom={zoom_level}")
        waveform = self._generate_waveform(audio_file_id, zoom_level)
        self._cache_waveform(audio_file_id, zoom_level, waveform)

        return waveform

    def _generate_waveform(self, audio_file_id: int, zoom_level: str):
        """Generate downsampled waveform for display"""
        from src.database.models import AudioFile

        # Get audio file metadata
        audio_file = self.session.query(AudioFile).get(audio_file_id)
        if not audio_file:
            raise ValueError(f"Audio file {audio_file_id} not found")

        # Load audio data
        audio_data, sample_rate = sf.read(audio_file.file_path, dtype='float32')

        # Convert stereo to mono for waveform (mix channels)
        if len(audio_data.shape) > 1:
            audio_data = audio_data.mean(axis=1)

        # Get decimation factor for this zoom level
        config = self.zoom_configs[zoom_level]
        decimation_factor = config['samples_per_pixel']

        # Peak-preserving decimation
        waveform = self._decimate_preserve_peaks(audio_data, decimation_factor)

        return waveform

    def _decimate_preserve_peaks(self, audio_data, factor):
        """Decimate audio while preserving peaks"""
        if factor <= 1:
            return audio_data

        # Calculate number of output samples
        num_samples = len(audio_data) // factor

        # For each window, store (min, max) to preserve peaks
        waveform = np.zeros((num_samples, 2), dtype=np.float32)

        for i in range(num_samples):
            start = i * factor
            end = min(start + factor, len(audio_data))
            window = audio_data[start:end]

            waveform[i, 0] = window.min()  # Negative peak
            waveform[i, 1] = window.max()  # Positive peak

        return waveform

    def _cache_waveform(self, audio_file_id, zoom_level, waveform):
        """Store waveform in database cache"""
        config = self.zoom_configs[zoom_level]

        cached = WaveformCache(
            audio_file_id=audio_file_id,
            zoom_level=zoom_level,
            waveform_data=pickle.dumps(waveform),
            samples_per_pixel=config['samples_per_pixel']
        )

        self.session.add(cached)
        self.session.commit()
```

**Canvas Rendering Optimization:**

```python
def render_to_canvas(self, canvas, waveform, width, height, color='#00AA00'):
    """Efficiently render waveform to tkinter canvas"""
    canvas.delete('all')

    # For large waveforms (>10K points), render to PIL image instead
    if len(waveform) > 10000:
        self._render_to_image(canvas, waveform, width, height, color)
        return

    # Build point list for single create_line() call
    points = []
    x_scale = width / len(waveform)
    y_scale = height / 2

    for i, (min_val, max_val) in enumerate(waveform):
        x = i * x_scale
        y_min = height/2 - min_val * y_scale
        y_max = height/2 - max_val * y_scale

        # Add vertical line for this sample
        points.extend([x, y_min, x, y_max])

    # Single canvas draw operation (fast)
    canvas.create_line(points, fill=color, width=1, capstyle=tk.BUTT)

def _render_to_image(self, canvas, waveform, width, height, color):
    """Render waveform to PIL image for better performance"""
    from PIL import Image, ImageDraw

    # Create image
    img = Image.new('RGB', (width, height), color='black')
    draw = ImageDraw.Draw(img)

    # Draw waveform
    x_scale = width / len(waveform)
    y_scale = height / 2

    for i, (min_val, max_val) in enumerate(waveform):
        x = int(i * x_scale)
        y_min = int(height/2 - min_val * y_scale)
        y_max = int(height/2 - max_val * y_scale)

        draw.line([(x, y_min), (x, y_max)], fill=color, width=1)

    # Convert to PhotoImage and display
    from PIL import ImageTk
    photo = ImageTk.PhotoImage(img)
    canvas.create_image(0, 0, image=photo, anchor=tk.NW)
    canvas.image = photo  # Keep reference to prevent garbage collection
```

### 9.4 Memory Management

**Waveform Cache Size Limits:**

```python
# src/database/db_manager.py

def prune_waveform_cache(session, max_size_mb=500):
    """Remove oldest waveform cache entries to stay under size limit"""
    from src.database.models import WaveformCache

    # Calculate current cache size
    total_size = session.query(
        func.sum(func.length(WaveformCache.waveform_data))
    ).scalar() or 0

    total_size_mb = total_size / (1024 * 1024)

    if total_size_mb > max_size_mb:
        logger.info(f"Waveform cache size ({total_size_mb:.1f} MB) exceeds limit, pruning...")

        # Delete oldest entries
        to_delete = session.query(WaveformCache).order_by(
            WaveformCache.generated_at.asc()
        ).limit(100).all()

        for entry in to_delete:
            session.delete(entry)

        session.commit()
        logger.info(f"Deleted {len(to_delete)} cache entries")
```

---

## 10. Client-Server Abstraction

### 10.1 Data Provider Interface (Complete)

```python
# src/database/data_provider.py

from abc import ABC, abstractmethod
from typing import List, Dict, Any, Optional
from datetime import datetime

class DataProvider(ABC):
    """
    Abstraction layer for data access (local SQLite or remote server)
    Enables seamless migration from Phase 1 (local) to Phase 2 (client-server)
    """

    # ========== Search Operations ==========

    @abstractmethod
    def search(self, query: str, filters: Optional[Dict[str, Any]] = None,
               limit: int = 1000, offset: int = 0) -> List[Any]:
        """
        Full-text search across audio files

        Args:
            query: Search query string (FTS5 syntax)
            filters: Additional filters (sample_rate, bit_depth, etc.)
            limit: Maximum results to return
            offset: Pagination offset

        Returns:
            List of AudioFile objects
        """
        pass

    @abstractmethod
    def get_audio_file(self, file_id: int) -> Optional[Any]:
        """Get single audio file by ID"""
        pass

    @abstractmethod
    def get_audio_files_by_path(self, paths: List[str]) -> List[Any]:
        """Get multiple audio files by file paths"""
        pass

    # ========== Scanning and Metadata ==========

    @abstractmethod
    def scan_directory(self, directory_path: str,
                      recursive: bool = True,
                      progress_callback = None) -> Dict[str, int]:
        """
        Scan directory for audio files and add to database

        Args:
            directory_path: Root directory to scan
            recursive: Include subdirectories
            progress_callback: Function(current, total) for progress updates

        Returns:
            Statistics: {'added': 10, 'updated': 5, 'failed': 2}
        """
        pass

    @abstractmethod
    def update_metadata(self, file_id: int, metadata: Dict[str, Any]) -> bool:
        """Update audio file metadata (user annotations)"""
        pass

    @abstractmethod
    def delete_files(self, file_ids: List[int]) -> int:
        """Delete files from database, return count deleted"""
        pass

    # ========== User Annotations ==========

    @abstractmethod
    def set_rating(self, file_id: int, rating: int) -> bool:
        """Set user rating (1-5 stars)"""
        pass

    @abstractmethod
    def set_favorite(self, file_id: int, is_favorite: bool) -> bool:
        """Toggle favorite status"""
        pass

    @abstractmethod
    def update_tags(self, file_id: int, tags: List[str]) -> bool:
        """Update user tags"""
        pass

    @abstractmethod
    def update_notes(self, file_id: int, notes: str) -> bool:
        """Update user notes"""
        pass

    # ========== Playlists ==========

    @abstractmethod
    def create_playlist(self, name: str, description: str = "") -> int:
        """Create new playlist, return playlist ID"""
        pass

    @abstractmethod
    def get_playlists(self) -> List[Any]:
        """Get all playlists"""
        pass

    @abstractmethod
    def add_to_playlist(self, playlist_id: int, file_ids: List[int]) -> bool:
        """Add files to playlist"""
        pass

    @abstractmethod
    def remove_from_playlist(self, playlist_id: int, file_ids: List[int]) -> bool:
        """Remove files from playlist"""
        pass

    # ========== Waveform Cache ==========

    @abstractmethod
    def get_waveform_cache(self, file_id: int, zoom_level: str) -> Optional[bytes]:
        """Get cached waveform data (pickled numpy array)"""
        pass

    @abstractmethod
    def set_waveform_cache(self, file_id: int, zoom_level: str,
                          waveform_data: bytes, samples_per_pixel: int) -> bool:
        """Store waveform cache"""
        pass

    # ========== Statistics and Reporting ==========

    @abstractmethod
    def get_database_stats(self) -> Dict[str, Any]:
        """
        Get database statistics

        Returns:
            {
                'total_files': 12345,
                'total_size_bytes': 1234567890,
                'total_duration_seconds': 12345.67,
                'sample_rate_distribution': {44100: 100, 48000: 200, ...},
                'bit_depth_distribution': {16: 50, 24: 250}
            }
        """
        pass

    @abstractmethod
    def get_scan_history(self, limit: int = 10) -> List[Any]:
        """Get recent scan sessions"""
        pass

    # ========== Database Management ==========

    @abstractmethod
    def optimize_database(self) -> bool:
        """Run VACUUM, ANALYZE, REINDEX operations"""
        pass

    @abstractmethod
    def check_integrity(self) -> bool:
        """Check database integrity"""
        pass

    @abstractmethod
    def close(self) -> None:
        """Close database connection / cleanup resources"""
        pass
```

### 10.2 Local Implementation (SQLite)

```python
# src/database/local_provider.py

from sqlalchemy.orm import Session
from src.database.data_provider import DataProvider
from src.database.models import AudioFile, WaveformCache, Playlist
from src.scanner.scanner_service import ScannerService
import pickle

class LocalDataProvider(DataProvider):
    """Local SQLite implementation of DataProvider"""

    def __init__(self, session: Session):
        self.session = session
        self.scanner = ScannerService(session)

    def search(self, query, filters=None, limit=1000, offset=0):
        # FTS5 search
        sql = """
            SELECT audio_files.*
            FROM audio_files
            JOIN audio_search ON audio_files.id = audio_search.rowid
            WHERE audio_search MATCH :query
            ORDER BY rank
            LIMIT :limit OFFSET :offset
        """

        results = self.session.execute(sql, {
            'query': query,
            'limit': limit,
            'offset': offset
        })

        return [AudioFile(**dict(row)) for row in results]

    def get_audio_file(self, file_id):
        return self.session.query(AudioFile).get(file_id)

    def scan_directory(self, directory_path, recursive=True, progress_callback=None):
        return self.scanner.scan_directory(directory_path, recursive, progress_callback)

    def update_metadata(self, file_id, metadata):
        file = self.session.query(AudioFile).get(file_id)
        if not file:
            return False

        for key, value in metadata.items():
            setattr(file, key, value)

        self.session.commit()
        return True

    # ... implement all other abstract methods
```

### 10.3 Remote Implementation (REST Client)

```python
# src/database/remote_provider.py

import requests
from src.database.data_provider import DataProvider

class RemoteDataProvider(DataProvider):
    """REST API client implementation for server mode (Phase 2)"""

    def __init__(self, api_url: str, auth_token: str = None):
        self.api_url = api_url.rstrip('/')
        self.auth_token = auth_token
        self.session = requests.Session()

        if auth_token:
            self.session.headers['Authorization'] = f"Bearer {auth_token}"

    def search(self, query, filters=None, limit=1000, offset=0):
        response = self.session.post(
            f"{self.api_url}/search",
            json={
                'query': query,
                'filters': filters,
                'limit': limit,
                'offset': offset
            }
        )
        response.raise_for_status()

        # Deserialize JSON to AudioFile objects
        from src.database.models import AudioFile
        return [AudioFile(**item) for item in response.json()['results']]

    def get_audio_file(self, file_id):
        response = self.session.get(f"{self.api_url}/files/{file_id}")
        response.raise_for_status()

        from src.database.models import AudioFile
        return AudioFile(**response.json())

    def scan_directory(self, directory_path, recursive=True, progress_callback=None):
        # Server-side scanning
        response = self.session.post(
            f"{self.api_url}/scan",
            json={
                'directory_path': directory_path,
                'recursive': recursive
            }
        )
        response.raise_for_status()
        return response.json()['statistics']

    # ... implement all other abstract methods
```

### 10.4 Factory Pattern for Provider Selection

```python
# src/database/provider_factory.py

from src.database.data_provider import DataProvider
from src.database.local_provider import LocalDataProvider
from src.database.remote_provider import RemoteDataProvider
from src.utils.config import get_config

def create_data_provider() -> DataProvider:
    """Factory function to create appropriate data provider"""
    config = get_config()

    if config.mode == 'local':
        # Phase 1: Local SQLite
        from src.database.db_manager import get_session
        session = get_session()
        return LocalDataProvider(session)

    elif config.mode == 'remote':
        # Phase 2: Remote server
        return RemoteDataProvider(
            api_url=config.server_url,
            auth_token=config.auth_token
        )

    else:
        raise ValueError(f"Invalid mode: {config.mode}")
```

---

## 11. Development Roadmap

### 11.1 18-Week Implementation Schedule

**Week 1-3: Core Infrastructure**
- Set up project structure (directory layout, requirements.txt)
- Implement database schema (SQLAlchemy models)
- Create service registry system (copy from Video_Explorer)
- Set up configuration management (Pydantic)
- Implement logging infrastructure (loguru)
- Set up i18n translation system (EN/CN base files)
- **Deliverable:** Basic application scaffold with database

**Week 4-6: Audio Processing**
- Implement WAV metadata extraction (wavinfo integration)
- Create audio file scanner with parallel processing
- Implement waveform cache generation (3-level)
- Set up audio playback system (python-sounddevice)
- Implement basic spectrum analysis (librosa FFT)
- **Deliverable:** Functional audio scanning and playback

**Week 7-8: Search and Indexing**
- Implement SQLite FTS5 search engine
- Create search query parser
- Implement advanced filter system
- Performance testing with 100K synthetic records
- **Deliverable:** <500ms search on 100K+ records

**Week 9-12: GUI Development**
- Create main window layout (tkinter)
- Implement folder tree panel
- Implement results list view
- Create waveform visualization panel
- Create spectrum analyzer panel
- Implement audio player controls
- Build status bar and progress indicators
- **Deliverable:** Fully functional GUI

**Week 13-15: Advanced Features**
- Implement drag-and-drop file sharing (TkinterDnD2)
- Build plugin system architecture
- Create 2 example plugins (color waveform, advanced spectrum)
- Implement layout customization and persistence
- Add keyboard shortcuts system
- **Deliverable:** Feature-complete application

**Week 16-17: Testing and Polish**
- Performance testing with 1M records
- UI responsiveness optimization
- Memory leak testing
- Cross-platform testing (Windows primary)
- User acceptance testing with beta users
- **Deliverable:** Production-ready v1.0

**Week 18: Server Interface Prep**
- Implement complete DataProvider abstraction
- Design REST API endpoints (specification only)
- Create migration documentation
- **Deliverable:** Phase 2 readiness

### 11.2 Technology Validation (Week 1)

**Critical Path Items to Validate Early:**

1. **SQLite FTS5 Performance Test**
   - Generate 1M synthetic audio file metadata records
   - Test search latency (target: <500ms for 95th percentile)
   - If failed: Evaluate DuckDB or Tantivy alternatives

2. **Waveform Rendering Performance Test**
   - Generate waveform for 10-minute WAV file
   - Test canvas rendering with 100K points
   - If slow: Implement PIL/Pillow bitmap rendering fallback

3. **Audio Playback Quality Test**
   - Test python-sounddevice with 96kHz/24-bit files
   - Verify no dropouts or glitches
   - Test drag-and-drop to Reaper/Pro Tools

**Go/No-Go Decision Point:** End of Week 1
- If any critical validation fails, adjust technology choices before proceeding

### 11.3 Milestone Deliverables

| Week | Milestone | Deliverable | Success Criteria |
|------|-----------|-------------|------------------|
| 3 | Infrastructure Complete | Running app with database | Can create database, no crashes |
| 6 | Audio Core Complete | Can scan and play audio | Scan 100 files, play WAV files |
| 8 | Search Complete | Functional search engine | <500ms search on 100K records |
| 12 | GUI Complete | Fully functional UI | All features accessible via GUI |
| 15 | Feature Complete | All P0 features done | Passes feature checklist |
| 17 | Release Candidate | Production build | Passes all tests, no critical bugs |

---

## Appendix A: Glossary and Terminology Guide

### A.1 Core Terminology

This glossary standardizes terminology used throughout all AudioLib Explorer documentation (PRD, Technical Architecture, API Design, Security Guidelines, Testing Strategy).

| Term | Definition | Usage Notes |
|------|-----------|-------------|
| **AudioLib Explorer** | Full product name | Always capitalize both words |
| **Audio file** | Any audio file in the library | Lowercase unless starting sentence |
| **BWF** | Broadcast Wave Format | Industry standard extension to WAV format |
| **DAW** | Digital Audio Workstation | Software like Pro Tools, Reaper, Ableton, FL Studio |
| **FTS5** | SQLite Full-Text Search 5 | SQLite's full-text search engine module |
| **Metadata** | Data about audio files (sample rate, bit depth, etc.) | One word, no hyphen |
| **P50/P95/P99** | 50th/95th/99th percentile | Performance metric percentiles |
| **Phase 1** | Local desktop application (current focus) | Client-only architecture |
| **Phase 2** | Client-server architecture (future) | Network-enabled version |
| **Search latency** | Time from query submission to result display | Measured in milliseconds (ms) |
| **Waveform** | Visual representation of audio amplitude over time | One word, no space |

### A.2 Performance Metrics Standardization

**Search Performance - Standard Expression:**
- **Target**: P95 latency <500ms for 1M+ records
- **Alternative phrasing**: "Sub-500ms search latency at 95th percentile"
- **DO NOT USE**: "sub-second", "fast search", "quick search" (too vague)

**Format Examples:**
```
✓ CORRECT:
- "P95 search latency <500ms for 1M+ records"
- "Search query execution in <500ms (P95) for 1M+ database"
- "95th percentile query latency under 500 milliseconds"

✗ INCORRECT:
- "sub-second search" (too vague, doesn't specify percentile)
- "< 500ms" with space before number
- "less than 500 ms" (spell out "milliseconds" or use "ms")
- "fast search" (subjective, not measurable)
```

**Database Size - Standard Expression:**
- **Target**: 1M+ records (one million or more)
- **Format**: Always use "M" for million, "K" for thousand
- **Examples**: "100K records", "1M+ records", "5M records"

**Playback Latency:**
- **Target**: <50ms from button click to audio output
- **Format**: "<50ms playback latency"

**UI Responsiveness:**
- **Target**: <100ms for UI actions (scroll, click)
- **Format**: "<100ms UI response time"

### A.3 Technical Term Conventions

**File Formats:**
- Use ALL CAPS with leading dot: `.WAV`, `.MP3`, `.FLAC`
- Exception: In code, use lowercase: `file.endswith('.wav')`

**Audio Technical Parameters:**
| Parameter | Format | Example |
|-----------|--------|---------|
| Sample rate | Number + kHz/Hz | 48kHz, 96000Hz |
| Bit depth | Number + -bit | 24-bit, 16-bit |
| Channels | Descriptive or number | stereo, mono, 5.1, 7.1 |
| Duration | Number + s/ms | 2.5s, 350ms |
| File size | Number + MB/GB | 15.3MB, 2.1GB |

**Database Terms:**
- **FTS5 index**: Not "full-text index" or "search index"
- **SQLAlchemy ORM**: Always include "ORM" when first mentioned
- **Session**: Database session (SQLAlchemy)
- **Query**: Database query OR search query (disambiguate with context)

### A.4 Chinese Translation Standards

| English | Chinese (Simplified) | Notes |
|---------|---------------------|-------|
| AudioLib Explorer | 音频库浏览器 | Official Chinese name |
| Audio library | 音频库 | Not 音频图书馆 |
| Metadata | 元数据 | Standard translation |
| Waveform | 波形 | Standard translation |
| Search latency | 搜索延迟 | Standard translation |
| P95 latency | P95延迟 / 第95百分位延迟 | Use P95 directly |
| DAW | DAW / 数字音频工作站 | Use DAW abbreviation |
| Sample rate | 采样率 | Standard translation |
| Bit depth | 位深度 | Standard translation |

### A.5 Abbreviation Guidelines

**Always spell out on first use:**
- BWF (Broadcast Wave Format)
- DAW (Digital Audio Workstation)
- FTS5 (SQLite Full-Text Search 5)
- ORM (Object-Relational Mapping)
- UI (User Interface)
- UX (User Experience)
- API (Application Programming Interface)
- REST (Representational State Transfer)

**Can use without expansion (industry standard):**
- WAV, MP3, FLAC (audio formats)
- JSON, XML (data formats)
- HTTP, HTTPS (protocols)
- SQLite, PostgreSQL (databases)
- GB, MB, KB (file sizes)
- ms, s (time units)

### A.6 Code Naming Conventions

**Python Modules:**
- `snake_case` for module names: `search_engine.py`, `metadata_extractor.py`
- `PascalCase` for class names: `SearchEngine`, `AudioFile`
- `snake_case` for function names: `extract_metadata()`, `scan_directory()`

**Database Tables:**
- `snake_case` for table names: `audio_files`, `project_references`
- `snake_case` for column names: `sample_rate`, `user_rating`

**UI Element Naming:**
- Use descriptive names: `search_button`, `results_listbox`
- Prefix with element type: `btn_play`, `lbl_status`, `txt_search`

### A.7 Documentation Style Guide

**Headings:**
- Use sentence case: "Database optimization strategies"
- NOT title case: "Database Optimization Strategies"
- Exception: Proper nouns like "AudioLib Explorer"

**Code Examples:**
- Always include file path comment: `# src/database/models.py`
- Include docstrings for public functions
- Use type hints for function signatures

**Performance Targets - Consistent Phrasing:**

```
STANDARD FORMAT:
- **Target**: [Metric] [Operator][Value] for [Context]
- **Measurement**: [How measured]
- **Rationale**: [Why this target]

EXAMPLE:
- **Target**: P95 search latency <500ms for 1M+ records
- **Measurement**: 95th percentile of 100 queries on populated test database
- **Rationale**: Sub-second response critical for user workflow state
```

### A.8 Common Inconsistencies to Avoid

| ❌ Incorrect | ✓ Correct | Reason |
|-------------|-----------|--------|
| wave form | waveform | One word |
| meta data | metadata | One word |
| meta-data | metadata | No hyphen |
| sub-second | <500ms (P95) | Be specific |
| < 500ms | <500ms | No space before number |
| fast search | P95 <500ms | Quantify performance |
| 1M records | 1M+ records | Use "+" to indicate "or more" |
| .wav file | .WAV file OR WAV file | Consistent capitalization |
| DAW project | DAW project file | "DAW project" is redundant |
| FTS index | FTS5 index | Specify version |

### A.9 Version References

When referring to software versions in documentation:

**Format**: `[Software] [Version]+`
- Python 3.11+
- SQLite 3.35+
- tkinter 8.6+
- librosa 0.10+
- wavinfo 2.1+

**DO NOT USE**:
- "Latest version" (becomes outdated)
- "Current version" (ambiguous)
- Version ranges unless necessary: "3.8-3.12" only if 3.13+ breaks compatibility

### A.10 Performance Target Quick Reference

| Metric | Target | Context |
|--------|--------|---------|
| **Search Latency (P50)** | <100ms | 50% of queries, 1M+ records |
| **Search Latency (P95)** | <500ms | 95% of queries, 1M+ records (PRIMARY METRIC) |
| **Search Latency (P99)** | <1000ms | 99% of queries, 1M+ records |
| **Playback Start** | <50ms | From button click to audio output |
| **UI Response** | <100ms | Button clicks, scroll, etc. |
| **Scan Throughput** | >100 files/sec | Metadata extraction on SSD |
| **Waveform Generation** | <200ms | 1-minute audio file, full resolution |
| **Database Startup** | <2s | Cold start, 1M records |

---

**END OF TECHNICAL ARCHITECTURE DOCUMENT**
# Current implementation note

The Rust workspace now supersedes this document's Python, SQLAlchemy ORM and tkinter implementation examples. See [Rust Architecture](Rust_Architecture.md) and [Rust Validation](Rust_Validation.md) for the current contracts, runtime, migrations and verification. Appendix A continues to define standardized terminology. Legacy Python source and its test scripts have been removed from the working tree; earlier committed versions preserve them. The original database and local config/audiolib_config.yaml remain read-only migration inputs.

