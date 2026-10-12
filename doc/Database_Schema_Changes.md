# Database Schema Changes: Directory Indexing with Closure Table

## Overview

This document describes the database schema changes required to implement **directory structure indexing** with **cross-indexing** capability for AudioLib Explorer. The design uses a **closure table pattern** to enable O(1) ancestor/descendant queries, essential for handling 1M+ records with sub-500ms query performance.

---

## 1. New Tables

### 1.1 Folders Table

The `folders` table stores directory hierarchy information with denormalized statistics for fast UI rendering.

```sql
CREATE TABLE folders (
    -- Primary identification
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_path TEXT NOT NULL UNIQUE,        -- Full normalized path (e.g., 'N:/Audio/SFX/Explosions')
    folder_name VARCHAR(255) NOT NULL,       -- Just the folder name (e.g., 'Explosions')

    -- Hierarchy relationships
    parent_id INTEGER,                        -- NULL for root folders (drive letters/mount points)
    depth INTEGER NOT NULL DEFAULT 0,         -- 0 for root, 1 for first level, etc.

    -- Cached statistics (denormalized for performance)
    file_count INTEGER NOT NULL DEFAULT 0,   -- Direct files in this folder only
    total_file_count INTEGER NOT NULL DEFAULT 0, -- Files including all descendants
    total_duration REAL DEFAULT 0,           -- Sum of all audio durations (seconds)
    total_size INTEGER DEFAULT 0,            -- Sum of all file sizes (bytes)

    -- Metadata
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    last_scanned DATETIME,                   -- Last time folder was scanned for changes

    -- Foreign key constraint
    FOREIGN KEY (parent_id) REFERENCES folders(id) ON DELETE CASCADE
);

-- Performance indexes
CREATE INDEX idx_folders_parent ON folders(parent_id);
CREATE INDEX idx_folders_path ON folders(folder_path);
CREATE INDEX idx_folders_depth ON folders(depth);
CREATE INDEX idx_folders_name ON folders(folder_name);
```

**Column Descriptions:**

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER | Auto-increment primary key |
| `folder_path` | TEXT | Full absolute path, unique constraint |
| `folder_name` | VARCHAR(255) | Display name (last component of path) |
| `parent_id` | INTEGER | Foreign key to parent folder (NULL for roots) |
| `depth` | INTEGER | Nesting level (0 = root) |
| `file_count` | INTEGER | Direct child file count (not recursive) |
| `total_file_count` | INTEGER | Recursive file count (includes all descendants) |
| `total_duration` | REAL | Sum of all descendant audio durations |
| `total_size` | INTEGER | Sum of all descendant file sizes |
| `created_at` | DATETIME | Record creation timestamp |
| `updated_at` | DATETIME | Last modification timestamp |
| `last_scanned` | DATETIME | Last filesystem scan timestamp |

### 1.2 Folder Closure Table

The closure table enables O(1) ancestor/descendant lookups by storing ALL path relationships, not just parent-child.

```sql
CREATE TABLE folder_closure (
    ancestor_id INTEGER NOT NULL,            -- The ancestor folder
    descendant_id INTEGER NOT NULL,          -- The descendant folder (can be same as ancestor)
    depth INTEGER NOT NULL DEFAULT 0,        -- Distance between ancestor and descendant

    PRIMARY KEY (ancestor_id, descendant_id),
    FOREIGN KEY (ancestor_id) REFERENCES folders(id) ON DELETE CASCADE,
    FOREIGN KEY (descendant_id) REFERENCES folders(id) ON DELETE CASCADE
);

-- Performance indexes for common query patterns
CREATE INDEX idx_closure_descendant ON folder_closure(descendant_id);
CREATE INDEX idx_closure_ancestor_depth ON folder_closure(ancestor_id, depth);
CREATE INDEX idx_closure_depth ON folder_closure(depth);
```

**How the Closure Table Works:**

For a folder hierarchy:
```
Root (id=1)
  └── SFX (id=2)
       └── Explosions (id=3)
            └── Large (id=4)
```

The closure table contains:
| ancestor_id | descendant_id | depth |
|-------------|---------------|-------|
| 1 | 1 | 0 | (self-reference)
| 2 | 2 | 0 | (self-reference)
| 3 | 3 | 0 | (self-reference)
| 4 | 4 | 0 | (self-reference)
| 1 | 2 | 1 | (Root → SFX)
| 1 | 3 | 2 | (Root → Explosions)
| 1 | 4 | 3 | (Root → Large)
| 2 | 3 | 1 | (SFX → Explosions)
| 2 | 4 | 2 | (SFX → Large)
| 3 | 4 | 1 | (Explosions → Large)

---

## 2. Audio Files Table Modification

### 2.1 Add folder_id Column

```sql
-- Add folder reference to audio_files table
ALTER TABLE audio_files ADD COLUMN folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL;

-- Create indexes for folder-based queries
CREATE INDEX idx_audio_files_folder ON audio_files(folder_id);

-- Composite indexes for cross-indexed queries with common filters
CREATE INDEX idx_audio_files_folder_sample_rate ON audio_files(folder_id, sample_rate);
CREATE INDEX idx_audio_files_folder_duration ON audio_files(folder_id, duration);
CREATE INDEX idx_audio_files_folder_channels ON audio_files(folder_id, channels);
CREATE INDEX idx_audio_files_folder_bit_depth ON audio_files(folder_id, bit_depth);
```

### 2.2 Updated AudioFile Model

```python
# src/database/models.py

class AudioFile(Base):
    """Audio file entity with folder relationship"""
    __tablename__ = 'audio_files'

    # ... existing columns ...

    # NEW: Folder relationship
    folder_id = Column(Integer, ForeignKey('folders.id', ondelete='SET NULL'), index=True)
    folder = relationship('Folder', back_populates='audio_files')

    __table_args__ = (
        # ... existing indexes ...
        Index('idx_audio_files_folder', 'folder_id'),
        Index('idx_audio_files_folder_sample_rate', 'folder_id', 'sample_rate'),
        Index('idx_audio_files_folder_duration', 'folder_id', 'duration'),
    )
```

---

## 3. ORM Models

### 3.1 Folder Model

```python
# src/database/models.py

from sqlalchemy import Column, Integer, String, Text, Float, DateTime, ForeignKey, Index
from sqlalchemy.orm import relationship
from datetime import datetime

class Folder(Base):
    """
    Folder entity for directory hierarchy indexing.

    Uses closure table pattern for O(1) ancestor/descendant queries.
    Statistics are denormalized for fast UI rendering.
    """
    __tablename__ = 'folders'

    # Primary identification
    id = Column(Integer, primary_key=True)
    folder_path = Column(Text, unique=True, nullable=False)
    folder_name = Column(String(255), nullable=False)

    # Hierarchy
    parent_id = Column(Integer, ForeignKey('folders.id', ondelete='CASCADE'))
    depth = Column(Integer, nullable=False, default=0)

    # Cached statistics
    file_count = Column(Integer, nullable=False, default=0)
    total_file_count = Column(Integer, nullable=False, default=0)
    total_duration = Column(Float, default=0)
    total_size = Column(Integer, default=0)

    # Timestamps
    created_at = Column(DateTime, default=datetime.utcnow)
    updated_at = Column(DateTime, default=datetime.utcnow, onupdate=datetime.utcnow)
    last_scanned = Column(DateTime)

    # Relationships
    parent = relationship('Folder', remote_side=[id], backref='children')
    audio_files = relationship('AudioFile', back_populates='folder')

    # Table configuration
    __table_args__ = (
        Index('idx_folders_parent', 'parent_id'),
        Index('idx_folders_path', 'folder_path'),
        Index('idx_folders_depth', 'depth'),
        Index('idx_folders_name', 'folder_name'),
    )

    def __repr__(self):
        return f"<Folder(id={self.id}, path='{self.folder_path}')>"

    @property
    def is_root(self) -> bool:
        """Check if this is a root folder"""
        return self.parent_id is None

    @property
    def display_name(self) -> str:
        """Get display name with file count"""
        return f"{self.folder_name} ({self.total_file_count} files)"
```

### 3.2 FolderClosure Model

```python
# src/database/models.py

class FolderClosure(Base):
    """
    Closure table for O(1) ancestor/descendant queries.

    Each row represents a path from ancestor to descendant.
    Includes self-references (ancestor_id = descendant_id, depth = 0).
    """
    __tablename__ = 'folder_closure'

    ancestor_id = Column(
        Integer,
        ForeignKey('folders.id', ondelete='CASCADE'),
        primary_key=True
    )
    descendant_id = Column(
        Integer,
        ForeignKey('folders.id', ondelete='CASCADE'),
        primary_key=True
    )
    depth = Column(Integer, nullable=False, default=0)

    # Relationships for ORM navigation
    ancestor = relationship('Folder', foreign_keys=[ancestor_id])
    descendant = relationship('Folder', foreign_keys=[descendant_id])

    __table_args__ = (
        Index('idx_closure_descendant', 'descendant_id'),
        Index('idx_closure_ancestor_depth', 'ancestor_id', 'depth'),
        Index('idx_closure_depth', 'depth'),
    )

    def __repr__(self):
        return f"<FolderClosure(ancestor={self.ancestor_id}, descendant={self.descendant_id}, depth={self.depth})>"
```

---

## 4. Migration Script

### 4.1 Alembic Migration

```python
# src/database/migrations/versions/006_add_folder_indexing.py
"""Add folder indexing with closure table for hierarchy queries

Revision ID: 006
Revises: 005
Create Date: 2025-11-25
"""

from alembic import op
import sqlalchemy as sa
from datetime import datetime

revision = '006'
down_revision = '005'

def upgrade():
    """Add folder tables and migrate existing data"""

    # 1. Create folders table
    op.create_table(
        'folders',
        sa.Column('id', sa.Integer(), primary_key=True, autoincrement=True),
        sa.Column('folder_path', sa.Text(), nullable=False, unique=True),
        sa.Column('folder_name', sa.String(255), nullable=False),
        sa.Column('parent_id', sa.Integer(), sa.ForeignKey('folders.id', ondelete='CASCADE')),
        sa.Column('depth', sa.Integer(), nullable=False, default=0),
        sa.Column('file_count', sa.Integer(), nullable=False, default=0),
        sa.Column('total_file_count', sa.Integer(), nullable=False, default=0),
        sa.Column('total_duration', sa.Float(), default=0),
        sa.Column('total_size', sa.Integer(), default=0),
        sa.Column('created_at', sa.DateTime(), server_default=sa.func.now()),
        sa.Column('updated_at', sa.DateTime(), server_default=sa.func.now()),
        sa.Column('last_scanned', sa.DateTime()),
    )

    # Folder indexes
    op.create_index('idx_folders_parent', 'folders', ['parent_id'])
    op.create_index('idx_folders_path', 'folders', ['folder_path'])
    op.create_index('idx_folders_depth', 'folders', ['depth'])
    op.create_index('idx_folders_name', 'folders', ['folder_name'])

    # 2. Create folder_closure table
    op.create_table(
        'folder_closure',
        sa.Column('ancestor_id', sa.Integer(),
                  sa.ForeignKey('folders.id', ondelete='CASCADE'),
                  primary_key=True),
        sa.Column('descendant_id', sa.Integer(),
                  sa.ForeignKey('folders.id', ondelete='CASCADE'),
                  primary_key=True),
        sa.Column('depth', sa.Integer(), nullable=False, default=0),
    )

    # Closure table indexes
    op.create_index('idx_closure_descendant', 'folder_closure', ['descendant_id'])
    op.create_index('idx_closure_ancestor_depth', 'folder_closure', ['ancestor_id', 'depth'])
    op.create_index('idx_closure_depth', 'folder_closure', ['depth'])

    # 3. Add folder_id to audio_files
    op.add_column('audio_files',
                  sa.Column('folder_id', sa.Integer(),
                           sa.ForeignKey('folders.id', ondelete='SET NULL')))

    # Audio files folder indexes
    op.create_index('idx_audio_files_folder', 'audio_files', ['folder_id'])
    op.create_index('idx_audio_files_folder_sample_rate', 'audio_files',
                    ['folder_id', 'sample_rate'])
    op.create_index('idx_audio_files_folder_duration', 'audio_files',
                    ['folder_id', 'duration'])

def downgrade():
    """Remove folder indexing"""
    # Drop audio_files indexes and column
    op.drop_index('idx_audio_files_folder_duration', 'audio_files')
    op.drop_index('idx_audio_files_folder_sample_rate', 'audio_files')
    op.drop_index('idx_audio_files_folder', 'audio_files')
    op.drop_column('audio_files', 'folder_id')

    # Drop folder_closure table
    op.drop_table('folder_closure')

    # Drop folders table
    op.drop_table('folders')
```

---

## 5. Data Population Script

### 5.1 Populate Folders from Existing Audio Files

```python
# src/database/migrations/populate_folders.py
"""Populate folder tables from existing audio_files data"""

from pathlib import Path
from typing import Dict, Set, Optional
import logging
from sqlalchemy import text

logger = logging.getLogger('audiolib.migration')

def populate_folders_from_existing_data(session) -> int:
    """
    Populate folders and folder_closure tables from existing audio_files.

    Returns:
        Number of folders created
    """
    logger.info("Starting folder population from existing audio files...")

    # Step 1: Extract all unique folder paths from audio_files
    result = session.execute(text("SELECT DISTINCT file_path FROM audio_files"))
    all_paths = [row[0] for row in result]

    if not all_paths:
        logger.info("No audio files found, skipping folder population")
        return 0

    # Step 2: Build set of unique folder paths
    folder_paths: Set[str] = set()
    for file_path in all_paths:
        parent = Path(file_path).parent
        # Traverse up to root, collecting all folder paths
        while str(parent) not in ('', '.', '/'):
            normalized = str(parent).replace('\\', '/')
            folder_paths.add(normalized)
            parent = parent.parent

    logger.info(f"Found {len(folder_paths)} unique folder paths")

    # Step 3: Insert folders in depth order (parents first)
    folder_id_map: Dict[str, int] = {}
    sorted_folders = sorted(folder_paths, key=lambda p: len(Path(p).parts))

    for folder_path in sorted_folders:
        path = Path(folder_path)
        parent_path = str(path.parent).replace('\\', '/')
        parent_id = folder_id_map.get(parent_path) if parent_path != '.' else None
        depth = len(path.parts) - 1  # Drive letter is depth 0
        folder_name = path.name or folder_path  # Handle root drives

        # Insert folder
        result = session.execute(
            text("""
                INSERT INTO folders (folder_path, folder_name, parent_id, depth)
                VALUES (:path, :name, :parent_id, :depth)
            """),
            {
                'path': folder_path,
                'name': folder_name,
                'parent_id': parent_id,
                'depth': depth
            }
        )

        # Get the inserted ID
        folder_id = session.execute(
            text("SELECT id FROM folders WHERE folder_path = :path"),
            {'path': folder_path}
        ).scalar()

        folder_id_map[folder_path] = folder_id

    logger.info(f"Created {len(folder_id_map)} folder records")

    # Step 4: Build closure table
    logger.info("Building closure table...")
    for folder_path, folder_id in folder_id_map.items():
        # Self-reference (depth 0)
        session.execute(
            text("""
                INSERT INTO folder_closure (ancestor_id, descendant_id, depth)
                VALUES (:id, :id, 0)
            """),
            {'id': folder_id}
        )

        # Ancestors (walk up the tree)
        path = Path(folder_path)
        depth = 1
        current_parent = str(path.parent).replace('\\', '/')

        while current_parent in folder_id_map:
            ancestor_id = folder_id_map[current_parent]
            session.execute(
                text("""
                    INSERT INTO folder_closure (ancestor_id, descendant_id, depth)
                    VALUES (:anc, :desc, :depth)
                """),
                {'anc': ancestor_id, 'desc': folder_id, 'depth': depth}
            )
            path = Path(current_parent)
            current_parent = str(path.parent).replace('\\', '/')
            depth += 1

    # Step 5: Update audio_files with folder_id
    logger.info("Updating audio_files with folder references...")
    for file_path in all_paths:
        parent_path = str(Path(file_path).parent).replace('\\', '/')
        if parent_path in folder_id_map:
            session.execute(
                text("""
                    UPDATE audio_files
                    SET folder_id = :folder_id
                    WHERE file_path = :file_path
                """),
                {'folder_id': folder_id_map[parent_path], 'file_path': file_path}
            )

    # Step 6: Update folder statistics
    logger.info("Calculating folder statistics...")
    update_all_folder_statistics(session)

    session.commit()
    logger.info("Folder population completed successfully")

    return len(folder_id_map)


def update_all_folder_statistics(session):
    """Update file_count, total_file_count, total_duration, total_size for all folders"""

    # Direct file counts (non-recursive)
    session.execute(text("""
        UPDATE folders SET file_count = (
            SELECT COUNT(*) FROM audio_files WHERE folder_id = folders.id
        )
    """))

    # Total counts including descendants (using closure table)
    session.execute(text("""
        UPDATE folders SET total_file_count = (
            SELECT COUNT(*)
            FROM audio_files af
            INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
            WHERE fc.ancestor_id = folders.id
        )
    """))

    # Total duration
    session.execute(text("""
        UPDATE folders SET total_duration = (
            SELECT COALESCE(SUM(af.duration), 0)
            FROM audio_files af
            INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
            WHERE fc.ancestor_id = folders.id
        )
    """))

    # Total size
    session.execute(text("""
        UPDATE folders SET total_size = (
            SELECT COALESCE(SUM(af.file_size), 0)
            FROM audio_files af
            INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
            WHERE fc.ancestor_id = folders.id
        )
    """))


def update_folder_statistics(session, folder_id: int):
    """Update statistics for a single folder and all its ancestors"""

    # Get all ancestors (including self)
    ancestors = session.execute(
        text("""
            SELECT ancestor_id FROM folder_closure
            WHERE descendant_id = :folder_id
        """),
        {'folder_id': folder_id}
    ).fetchall()

    for (ancestor_id,) in ancestors:
        # Update direct file count
        session.execute(text("""
            UPDATE folders SET
                file_count = (SELECT COUNT(*) FROM audio_files WHERE folder_id = :id),
                total_file_count = (
                    SELECT COUNT(*) FROM audio_files af
                    INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
                    WHERE fc.ancestor_id = :id
                ),
                total_duration = (
                    SELECT COALESCE(SUM(af.duration), 0) FROM audio_files af
                    INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
                    WHERE fc.ancestor_id = :id
                ),
                total_size = (
                    SELECT COALESCE(SUM(af.file_size), 0) FROM audio_files af
                    INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
                    WHERE fc.ancestor_id = :id
                ),
                updated_at = CURRENT_TIMESTAMP
            WHERE id = :id
        """), {'id': ancestor_id})
```

---

## 6. Closure Table Management

### 6.1 Insert New Folder with Closure Entries

```python
def create_folder_with_closure(session, folder_path: str, parent_id: Optional[int] = None) -> int:
    """
    Create a new folder and populate closure table entries.

    Args:
        session: Database session
        folder_path: Full path to the folder
        parent_id: Parent folder ID (None for root folders)

    Returns:
        New folder ID
    """
    from pathlib import Path

    path = Path(folder_path)
    folder_name = path.name or folder_path
    depth = len(path.parts) - 1

    # Insert folder
    session.execute(
        text("""
            INSERT INTO folders (folder_path, folder_name, parent_id, depth)
            VALUES (:path, :name, :parent_id, :depth)
        """),
        {'path': folder_path, 'name': folder_name, 'parent_id': parent_id, 'depth': depth}
    )

    # Get new folder ID
    folder_id = session.execute(
        text("SELECT id FROM folders WHERE folder_path = :path"),
        {'path': folder_path}
    ).scalar()

    # Add self-reference to closure table
    session.execute(
        text("""
            INSERT INTO folder_closure (ancestor_id, descendant_id, depth)
            VALUES (:id, :id, 0)
        """),
        {'id': folder_id}
    )

    # Copy parent's ancestors with incremented depth
    if parent_id:
        session.execute(
            text("""
                INSERT INTO folder_closure (ancestor_id, descendant_id, depth)
                SELECT fc.ancestor_id, :new_id, fc.depth + 1
                FROM folder_closure fc
                WHERE fc.descendant_id = :parent_id
            """),
            {'new_id': folder_id, 'parent_id': parent_id}
        )

    return folder_id
```

### 6.2 Delete Folder (Cascade via Foreign Keys)

```python
def delete_folder(session, folder_id: int, delete_files: bool = False):
    """
    Delete a folder and optionally its files.

    Closure table entries are automatically deleted via ON DELETE CASCADE.

    Args:
        session: Database session
        folder_id: Folder to delete
        delete_files: If True, delete associated audio files
    """
    if delete_files:
        # Delete all audio files in folder and descendants
        session.execute(
            text("""
                DELETE FROM audio_files
                WHERE folder_id IN (
                    SELECT descendant_id FROM folder_closure
                    WHERE ancestor_id = :folder_id
                )
            """),
            {'folder_id': folder_id}
        )
    else:
        # Set folder_id to NULL for orphaned files
        session.execute(
            text("""
                UPDATE audio_files SET folder_id = NULL
                WHERE folder_id IN (
                    SELECT descendant_id FROM folder_closure
                    WHERE ancestor_id = :folder_id
                )
            """),
            {'folder_id': folder_id}
        )

    # Delete folder (closure entries cascade automatically)
    session.execute(
        text("DELETE FROM folders WHERE id = :folder_id"),
        {'folder_id': folder_id}
    )
```

---

## 7. Query Examples

### 7.1 Get All Files in Folder (Including Subfolders)

```sql
-- O(1) lookup using closure table
SELECT af.*
FROM audio_files af
INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
WHERE fc.ancestor_id = :folder_id;
```

### 7.2 Get Direct Children of a Folder

```sql
SELECT * FROM folders
WHERE parent_id = :parent_id
ORDER BY folder_name;
```

### 7.3 Get Folder Breadcrumb (All Ancestors)

```sql
SELECT f.*
FROM folders f
INNER JOIN folder_closure fc ON fc.ancestor_id = f.id
WHERE fc.descendant_id = :folder_id
ORDER BY fc.depth DESC;
```

### 7.4 Get Folder with Filtered File Count

```sql
SELECT
    f.*,
    (
        SELECT COUNT(*)
        FROM audio_files af
        INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
        INNER JOIN audio_search ON audio_search.rowid = af.id
        WHERE fc.ancestor_id = f.id
          AND audio_search MATCH :query
    ) as filtered_count
FROM folders f
WHERE f.parent_id = :parent_id;
```

---

## 8. Performance Expectations

| Query Type | Without Closure Table | With Closure Table |
|------------|----------------------|-------------------|
| Files in folder + subfolders | O(n) LIKE query | O(1) JOIN |
| Folder ancestor lookup | O(d) recursive | O(1) JOIN |
| Folder descendant count | O(n) recursive | O(1) aggregate |
| Cross-indexed search | Not possible | O(log n) FTS5 + JOIN |

**Benchmarks (1M records):**
- Files in folder query: <50ms (vs 500ms+ with LIKE)
- Folder tree expansion: <30ms
- Cross-indexed search: <500ms P95
