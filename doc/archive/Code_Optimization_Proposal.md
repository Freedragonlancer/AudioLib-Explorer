# Code Optimization Proposal: AudioLib Explorer

## Overview

This document provides detailed code optimization recommendations for AudioLib Explorer, targeting P95 search latency <500ms for 1M+ records with balanced performance for both keyword search and directory navigation.

---

## 1. Current Architecture Analysis

### 1.1 Existing Components

| Component | File | Current State | Optimization Priority |
|-----------|------|---------------|----------------------|
| DatabaseManager | `src/database/db_manager.py` | Good PRAGMA settings | Medium |
| DataProvider | `src/database/data_provider.py` | Solid abstraction | High (add folder ops) |
| SearchEngine | `src/search/search_engine.py` | FTS5 + caching | High (add cross-index) |
| ScannerService | `src/scanner/scanner_service.py` | Multi-threaded | High (add folder creation) |
| ServiceBridge | `src/gui_qt/services/service_bridge.py` | Qt signals | Medium (add folder signals) |

### 1.2 Performance Bottlenecks Identified

1. **No folder indexing** - Path prefix queries use LIKE which is O(n)
2. **No cross-indexing** - Cannot combine FTS5 search with folder filter
3. **Statistics recalculated** - COUNT(*) queries on each tree expand
4. **Single-threaded folder tree** - UI blocks during tree population

---

## 2. Database Layer Optimizations

### 2.1 Add Folder Operations to DataProvider

```python
# src/database/data_provider.py

from abc import abstractmethod
from typing import List, Optional, Dict, Any

class DataProvider(ABC):
    """Extended DataProvider interface with folder operations"""

    # --- Existing audio file methods ---
    # ... (keep existing methods)

    # --- NEW: Folder Operations ---

    @abstractmethod
    def get_folder(self, folder_id: int) -> Optional['Folder']:
        """Get folder by ID"""
        pass

    @abstractmethod
    def get_folder_by_path(self, folder_path: str) -> Optional['Folder']:
        """Get folder by absolute path"""
        pass

    @abstractmethod
    def get_root_folders(self) -> List['Folder']:
        """Get all root-level folders (no parent)"""
        pass

    @abstractmethod
    def get_child_folders(self, parent_id: int) -> List['Folder']:
        """Get direct children of a folder"""
        pass

    @abstractmethod
    def get_folder_ancestors(self, folder_id: int) -> List['Folder']:
        """Get all ancestors (for breadcrumb navigation)"""
        pass

    @abstractmethod
    def get_descendant_folder_ids(self, folder_id: int) -> List[int]:
        """Get all descendant folder IDs (via closure table)"""
        pass

    @abstractmethod
    def create_folder(self, folder_path: str, parent_id: Optional[int] = None) -> 'Folder':
        """Create folder with automatic closure table population"""
        pass

    @abstractmethod
    def delete_folder(self, folder_id: int, cascade_files: bool = False) -> bool:
        """Delete folder (closure entries cascade automatically)"""
        pass

    @abstractmethod
    def get_folder_file_count(
        self,
        folder_id: int,
        include_subfolders: bool = True,
        query: Optional[str] = None,
        filters: Optional[Dict] = None
    ) -> int:
        """Get file count with optional filtering"""
        pass

    @abstractmethod
    def update_folder_statistics(self, folder_id: int) -> None:
        """Recalculate and update folder statistics"""
        pass

    @abstractmethod
    def update_all_folder_statistics(self) -> None:
        """Recalculate statistics for all folders (batch operation)"""
        pass
```

### 2.2 LocalDataProvider Folder Implementation

```python
# src/database/data_provider.py

class LocalDataProvider(DataProvider):
    """SQLite implementation with folder operations"""

    def get_folder(self, folder_id: int) -> Optional[Folder]:
        with self.get_session() as session:
            return session.query(Folder).get(folder_id)

    def get_folder_by_path(self, folder_path: str) -> Optional[Folder]:
        with self.get_session() as session:
            return session.query(Folder).filter(
                Folder.folder_path == folder_path
            ).first()

    def get_root_folders(self) -> List[Folder]:
        with self.get_session() as session:
            return session.query(Folder).filter(
                Folder.parent_id.is_(None)
            ).order_by(Folder.folder_name).all()

    def get_child_folders(self, parent_id: int) -> List[Folder]:
        with self.get_session() as session:
            return session.query(Folder).filter(
                Folder.parent_id == parent_id
            ).order_by(Folder.folder_name).all()

    def get_folder_ancestors(self, folder_id: int) -> List[Folder]:
        """Get ancestors using closure table (O(1) query)"""
        with self.get_session() as session:
            from sqlalchemy import text
            result = session.execute(text("""
                SELECT f.*
                FROM folders f
                INNER JOIN folder_closure fc ON fc.ancestor_id = f.id
                WHERE fc.descendant_id = :folder_id
                ORDER BY fc.depth DESC
            """), {'folder_id': folder_id})

            return [
                session.query(Folder).get(row[0])
                for row in result.fetchall()
            ]

    def get_descendant_folder_ids(self, folder_id: int) -> List[int]:
        """Get all descendant IDs using closure table"""
        with self.get_session() as session:
            from sqlalchemy import text
            result = session.execute(text("""
                SELECT descendant_id
                FROM folder_closure
                WHERE ancestor_id = :folder_id AND depth > 0
            """), {'folder_id': folder_id})
            return [row[0] for row in result.fetchall()]

    def create_folder(self, folder_path: str, parent_id: Optional[int] = None) -> Folder:
        """Create folder with closure table entries"""
        from pathlib import Path

        with self.get_session() as session:
            path = Path(folder_path)
            folder = Folder(
                folder_path=folder_path,
                folder_name=path.name or folder_path,
                parent_id=parent_id,
                depth=len(path.parts) - 1
            )
            session.add(folder)
            session.flush()  # Get ID

            # Add self-reference to closure table
            from sqlalchemy import text
            session.execute(text("""
                INSERT INTO folder_closure (ancestor_id, descendant_id, depth)
                VALUES (:id, :id, 0)
            """), {'id': folder.id})

            # Copy parent's ancestors with incremented depth
            if parent_id:
                session.execute(text("""
                    INSERT INTO folder_closure (ancestor_id, descendant_id, depth)
                    SELECT fc.ancestor_id, :new_id, fc.depth + 1
                    FROM folder_closure fc
                    WHERE fc.descendant_id = :parent_id
                """), {'new_id': folder.id, 'parent_id': parent_id})

            session.commit()
            return folder

    def update_folder_statistics(self, folder_id: int) -> None:
        """Update single folder and all ancestors"""
        with self.get_session() as session:
            from sqlalchemy import text

            # Get all ancestors (including self)
            ancestors = session.execute(text("""
                SELECT ancestor_id FROM folder_closure
                WHERE descendant_id = :folder_id
            """), {'folder_id': folder_id}).fetchall()

            for (ancestor_id,) in ancestors:
                session.execute(text("""
                    UPDATE folders SET
                        file_count = (
                            SELECT COUNT(*) FROM audio_files
                            WHERE folder_id = :id
                        ),
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

            session.commit()
```

---

## 3. Search Engine Optimizations

### 3.1 Add Cross-Index Search Method

```python
# src/search/search_engine.py

def search_with_folder(
    self,
    query: Optional[str] = None,
    folder_id: Optional[int] = None,
    include_subfolders: bool = True,
    filters: Optional[Dict[str, Any]] = None,
    limit: int = 100,
    offset: int = 0,
    order_by: str = 'rank'
) -> Tuple[List[SearchResult], int]:
    """
    Cross-indexed search with simultaneous keyword + folder filtering.

    Performance: Uses closure table JOIN for O(1) subfolder inclusion.
    Caching: Results cached by (query, folder_id, filters, limit, offset).
    """
    # See Cross_Index_Implementation.md for full implementation
    pass
```

### 3.2 Improve Cache Efficiency

```python
# src/search/search_engine.py

class SearchCache:
    """Improved LRU cache with size-based eviction"""

    def __init__(self, max_size_mb: int = 50, max_entries: int = 1000, ttl_seconds: int = 300):
        self._cache: OrderedDict[str, CacheEntry] = OrderedDict()
        self._max_size = max_size_mb * 1024 * 1024
        self._max_entries = max_entries
        self._ttl = timedelta(seconds=ttl_seconds)
        self._current_size = 0
        self._lock = threading.RLock()

    def get(self, key: str) -> Optional[Any]:
        with self._lock:
            if key not in self._cache:
                return None

            entry = self._cache[key]

            # Check TTL
            if datetime.now() - entry.timestamp > self._ttl:
                self._remove(key)
                return None

            # Move to end (LRU)
            self._cache.move_to_end(key)
            return entry.value

    def set(self, key: str, value: Any, size_bytes: int = 0):
        with self._lock:
            # Remove if exists
            if key in self._cache:
                self._remove(key)

            # Evict if needed
            while (len(self._cache) >= self._max_entries or
                   self._current_size + size_bytes > self._max_size):
                if not self._cache:
                    break
                self._remove_oldest()

            # Add new entry
            self._cache[key] = CacheEntry(
                value=value,
                timestamp=datetime.now(),
                size=size_bytes
            )
            self._current_size += size_bytes

    def _remove(self, key: str):
        if key in self._cache:
            self._current_size -= self._cache[key].size
            del self._cache[key]

    def _remove_oldest(self):
        if self._cache:
            oldest_key = next(iter(self._cache))
            self._remove(oldest_key)
```

---

## 4. Scanner Service Optimizations

### 4.1 Folder Creation During Scan

```python
# src/scanner/scanner_service.py

class ScannerService:
    """Enhanced scanner with folder creation"""

    def __init__(self, data_provider, search_engine, folder_cache):
        self.data_provider = data_provider
        self.search_engine = search_engine
        self.folder_cache = folder_cache
        self._folder_path_cache: Dict[str, int] = {}  # path -> folder_id

    def _ensure_folder_hierarchy(self, file_path: str) -> int:
        """
        Ensure all folders in path exist, return leaf folder_id.

        Uses in-memory cache to avoid repeated database queries.
        """
        from pathlib import Path

        parent_path = str(Path(file_path).parent).replace('\\', '/')

        # Check memory cache first
        if parent_path in self._folder_path_cache:
            return self._folder_path_cache[parent_path]

        # Check database
        folder = self.data_provider.get_folder_by_path(parent_path)
        if folder:
            self._folder_path_cache[parent_path] = folder.id
            return folder.id

        # Create folder hierarchy (recursive)
        folders_to_create = []
        current = Path(parent_path)

        while str(current) not in ('', '.', '/'):
            current_str = str(current).replace('\\', '/')
            if current_str in self._folder_path_cache:
                break
            existing = self.data_provider.get_folder_by_path(current_str)
            if existing:
                self._folder_path_cache[current_str] = existing.id
                break
            folders_to_create.append(current_str)
            current = current.parent

        # Create folders top-down
        folders_to_create.reverse()
        parent_id = self._folder_path_cache.get(str(Path(folders_to_create[0]).parent).replace('\\', '/')) if folders_to_create else None

        for folder_path in folders_to_create:
            folder = self.data_provider.create_folder(folder_path, parent_id)
            self._folder_path_cache[folder_path] = folder.id
            parent_id = folder.id

        return self._folder_path_cache[parent_path]

    def _process_file(self, file_path: Path) -> Tuple[bool, bool]:
        """Process single file with folder assignment"""
        try:
            # Ensure folder exists
            folder_id = self._ensure_folder_hierarchy(str(file_path))

            # Extract metadata
            metadata = self.metadata_extractor.extract(str(file_path))
            if not metadata:
                return False, False

            # Add folder_id to metadata
            metadata['folder_id'] = folder_id

            # Create or update audio file
            existing = self.data_provider.get_audio_file_by_path(str(file_path))
            self.data_provider.create_or_update_audio_file(
                file_path=str(file_path),
                metadata=metadata
            )

            return True, existing is not None

        except Exception as e:
            logger.error(f"Error processing {file_path}: {e}")
            return False, False

    def _on_scan_complete(self, root_path: str):
        """Post-scan cleanup and statistics update"""
        # Clear scan cache
        self._folder_path_cache.clear()

        # Update folder statistics for scanned tree
        root_folder = self.data_provider.get_folder_by_path(root_path)
        if root_folder:
            self.data_provider.update_folder_statistics(root_folder.id)

        # Invalidate search cache
        self.search_engine.invalidate_cache()

        # Invalidate folder statistics cache
        self.folder_cache.invalidate_all()
```

### 4.2 Batch Processing Optimization

```python
# src/scanner/scanner_service.py

def _process_files_batch(self, files: List[Path], batch_size: int = 100):
    """Process files in batches for better database performance"""

    with self.data_provider.get_session() as session:
        for i in range(0, len(files), batch_size):
            batch = files[i:i + batch_size]

            for file_path in batch:
                try:
                    folder_id = self._ensure_folder_hierarchy(str(file_path))
                    metadata = self.metadata_extractor.extract(str(file_path))

                    if metadata:
                        metadata['folder_id'] = folder_id
                        self.data_provider.create_or_update_audio_file(
                            file_path=str(file_path),
                            metadata=metadata
                        )
                except Exception as e:
                    logger.warning(f"Error processing {file_path}: {e}")

            # Commit batch
            session.commit()

            # Update progress
            if self.progress_callback:
                self.progress_callback(i + len(batch), len(files), str(batch[-1]))
```

---

## 5. Service Bridge Optimizations

### 5.1 Add Folder-Related Signals

```python
# src/gui_qt/services/service_bridge.py

from PyQt6.QtCore import QObject, pyqtSignal, QThread

class ServiceBridge(QObject):
    """Extended with folder operations"""

    # Existing signals
    searchCompleted = pyqtSignal(list, int)  # (results, total_count)
    searchError = pyqtSignal(str)
    scanProgress = pyqtSignal(int, int, str)  # (current, total, filename)
    scanCompleted = pyqtSignal(int)

    # NEW: Folder signals
    folderTreeLoaded = pyqtSignal(list)  # List of root folders
    childFoldersLoaded = pyqtSignal(int, list)  # (parent_id, children)
    folderSelected = pyqtSignal(int, str)  # (folder_id, folder_path)
    folderStatisticsUpdated = pyqtSignal(int, int)  # (folder_id, file_count)

    def __init__(self):
        super().__init__()
        self._search_engine = None
        self._data_provider = None
        self._folder_cache = None

        # Debounce timer for search
        self._search_timer = QTimer()
        self._search_timer.setSingleShot(True)
        self._search_timer.setInterval(300)
        self._search_timer.timeout.connect(self._execute_pending_search)
        self._pending_search = None

    def search_debounced(
        self,
        query: str,
        folder_id: Optional[int] = None,
        filters: Optional[Dict] = None
    ):
        """Debounced search - waits 300ms after last call"""
        self._pending_search = (query, folder_id, filters)
        self._search_timer.start()

    def _execute_pending_search(self):
        """Execute the pending debounced search"""
        if self._pending_search:
            query, folder_id, filters = self._pending_search
            self._pending_search = None

            # Run in worker thread
            worker = SearchWorker(self._search_engine, query, folder_id, filters)
            worker.finished.connect(lambda r, t: self.searchCompleted.emit(r, t))
            worker.error.connect(self.searchError.emit)
            worker.start()

    def load_folder_tree(self, parent_id: Optional[int] = None):
        """Load folder tree (async)"""
        worker = FolderTreeWorker(self._data_provider, parent_id)
        if parent_id is None:
            worker.finished.connect(self.folderTreeLoaded.emit)
        else:
            worker.finished.connect(lambda f: self.childFoldersLoaded.emit(parent_id, f))
        worker.start()

    def get_folder_breadcrumb(self, folder_id: int) -> List[Dict]:
        """Get breadcrumb (synchronous, fast due to closure table)"""
        return self._data_provider.get_folder_ancestors(folder_id)


class SearchWorker(QThread):
    """Background worker for cross-indexed search"""

    finished = pyqtSignal(list, int)
    error = pyqtSignal(str)

    def __init__(self, search_engine, query, folder_id, filters):
        super().__init__()
        self.search_engine = search_engine
        self.query = query
        self.folder_id = folder_id
        self.filters = filters

    def run(self):
        try:
            results, total = self.search_engine.search_with_folder(
                query=self.query,
                folder_id=self.folder_id,
                filters=self.filters
            )
            self.finished.emit(results, total)
        except Exception as e:
            self.error.emit(str(e))


class FolderTreeWorker(QThread):
    """Background worker for folder tree loading"""

    finished = pyqtSignal(list)
    error = pyqtSignal(str)

    def __init__(self, data_provider, parent_id):
        super().__init__()
        self.data_provider = data_provider
        self.parent_id = parent_id

    def run(self):
        try:
            if self.parent_id is None:
                folders = self.data_provider.get_root_folders()
            else:
                folders = self.data_provider.get_child_folders(self.parent_id)

            # Convert to dicts for Qt signal
            result = [
                {
                    'id': f.id,
                    'name': f.folder_name,
                    'path': f.folder_path,
                    'file_count': f.total_file_count,
                    'has_children': len(f.children) > 0 if hasattr(f, 'children') else True
                }
                for f in folders
            ]
            self.finished.emit(result)
        except Exception as e:
            self.error.emit(str(e))
```

---

## 6. Memory Optimization

### 6.1 Virtual Table Model for Large Result Sets

```python
# src/gui_qt/models/virtual_table_model.py

from PyQt6.QtCore import QAbstractTableModel, Qt, QModelIndex
from typing import List, Any

class VirtualAudioTableModel(QAbstractTableModel):
    """
    Virtual table model for efficient display of large result sets.

    Only loads visible rows + buffer, fetches more on scroll.
    """

    BUFFER_SIZE = 100  # Rows to fetch beyond visible area

    def __init__(self, search_engine, parent=None):
        super().__init__(parent)
        self.search_engine = search_engine
        self._cache: Dict[int, SearchResult] = {}
        self._total_count = 0
        self._current_query = None
        self._current_folder_id = None
        self._current_filters = None

        self._columns = [
            ('filename', 'Filename'),
            ('duration', 'Duration'),
            ('sample_rate', 'Sample Rate'),
            ('bit_depth', 'Bit Depth'),
            ('channels', 'Channels'),
            ('file_size', 'Size'),
            ('folder_path', 'Folder'),
        ]

    def rowCount(self, parent=QModelIndex()) -> int:
        return self._total_count

    def columnCount(self, parent=QModelIndex()) -> int:
        return len(self._columns)

    def data(self, index: QModelIndex, role: int = Qt.ItemDataRole.DisplayRole):
        if not index.isValid():
            return None

        if role == Qt.ItemDataRole.DisplayRole:
            row = index.row()
            col = index.column()

            # Fetch if not in cache
            if row not in self._cache:
                self._fetch_range(row)

            if row in self._cache:
                result = self._cache[row]
                col_name = self._columns[col][0]
                return self._format_value(col_name, getattr(result, col_name, None))

        return None

    def _fetch_range(self, center_row: int):
        """Fetch rows around the requested row"""
        start = max(0, center_row - self.BUFFER_SIZE)
        end = min(self._total_count, center_row + self.BUFFER_SIZE)

        results, _ = self.search_engine.search_with_folder(
            query=self._current_query,
            folder_id=self._current_folder_id,
            filters=self._current_filters,
            limit=end - start,
            offset=start
        )

        for i, result in enumerate(results):
            self._cache[start + i] = result

    def set_search(self, query: str, folder_id: int, filters: Dict):
        """Update search parameters and refresh"""
        self.beginResetModel()

        self._current_query = query
        self._current_folder_id = folder_id
        self._current_filters = filters
        self._cache.clear()

        # Get total count
        _, self._total_count = self.search_engine.search_with_folder(
            query=query,
            folder_id=folder_id,
            filters=filters,
            limit=1,
            offset=0
        )

        self.endResetModel()

    def _format_value(self, col_name: str, value: Any) -> str:
        """Format value for display"""
        if value is None:
            return ''
        if col_name == 'duration':
            return f"{value:.2f}s"
        if col_name == 'sample_rate':
            return f"{value // 1000}kHz"
        if col_name == 'file_size':
            return self._format_size(value)
        return str(value)

    def _format_size(self, size: int) -> str:
        """Format file size"""
        for unit in ['B', 'KB', 'MB', 'GB']:
            if size < 1024:
                return f"{size:.1f} {unit}"
            size /= 1024
        return f"{size:.1f} TB"
```

---

## 7. Index Optimization Summary

### 7.1 Recommended Indexes

| Table | Index | Columns | Purpose |
|-------|-------|---------|---------|
| folders | idx_folders_parent | parent_id | Child folder queries |
| folders | idx_folders_path | folder_path | Path lookup |
| folders | idx_folders_depth | depth | Level-based queries |
| folder_closure | idx_closure_descendant | descendant_id | "Files in folder" |
| folder_closure | idx_closure_ancestor_depth | ancestor_id, depth | Breadcrumb queries |
| audio_files | idx_audio_files_folder | folder_id | Basic folder filter |
| audio_files | idx_audio_files_folder_sample_rate | folder_id, sample_rate | Filtered queries |
| audio_files | idx_audio_files_folder_duration | folder_id, duration | Duration filtering |

### 7.2 SQLite PRAGMA Recommendations

```python
# Existing PRAGMAs (already in db_manager.py) - verified optimal
PRAGMA journal_mode=WAL;          # Concurrent reads during writes
PRAGMA synchronous=NORMAL;        # Balance safety/speed
PRAGMA cache_size=-10000;         # 10MB cache
PRAGMA mmap_size=268435456;       # 256MB memory-mapped I/O
PRAGMA temp_store=MEMORY;         # Temp tables in RAM
PRAGMA page_size=4096;            # 4KB pages

# NEW: Additional optimizations
PRAGMA wal_autocheckpoint=1000;   # Checkpoint every 1000 pages
PRAGMA busy_timeout=5000;         # 5 second busy timeout
PRAGMA foreign_keys=ON;           # Enforce FK constraints
```

---

## 8. Performance Validation

### 8.1 Benchmark Script

```python
# tests/performance/cross_index_benchmark.py

import time
from statistics import mean, quantiles

def benchmark_cross_index_search(search_engine, iterations=100):
    """Benchmark cross-indexed search performance"""

    test_queries = [
        ("explosion", 1, {}),  # Simple keyword + folder
        ("*", 1, {'sample_rate': 48000}),  # Folder + filter
        ("impact OR crash", 1, {'min_duration': 1.0}),  # Complex
    ]

    results = []

    for query, folder_id, filters in test_queries:
        times = []
        for _ in range(iterations):
            start = time.perf_counter()
            search_engine.search_with_folder(
                query=query,
                folder_id=folder_id,
                filters=filters,
                limit=100
            )
            elapsed = (time.perf_counter() - start) * 1000  # ms
            times.append(elapsed)

        percentiles = quantiles(times, n=100)
        results.append({
            'query': query,
            'folder_id': folder_id,
            'filters': filters,
            'mean': mean(times),
            'p50': percentiles[49],
            'p95': percentiles[94],
            'p99': percentiles[98],
        })

    return results


def validate_performance_targets(results):
    """Check if performance meets targets"""
    for r in results:
        if r['p95'] > 500:
            print(f"FAIL: P95 {r['p95']:.0f}ms > 500ms for {r['query']}")
            return False
        print(f"PASS: P95 {r['p95']:.0f}ms for {r['query']}")
    return True
```

---

## 9. Implementation Checklist

- [ ] Add Folder and FolderClosure models to `models.py`
- [ ] Add folder_id column to AudioFile model
- [ ] Extend DataProvider with folder operations
- [ ] Implement LocalDataProvider folder methods
- [ ] Add search_with_folder to SearchEngine
- [ ] Implement FolderStatisticsCache
- [ ] Modify ScannerService for folder creation
- [ ] Add folder signals to ServiceBridge
- [ ] Create VirtualAudioTableModel
- [ ] Run performance validation benchmark
