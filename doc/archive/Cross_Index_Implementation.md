# Cross-Index Implementation: Query Patterns and Caching Strategy

## Overview

This document describes the implementation of **cross-indexed search** - the ability to simultaneously filter audio files by **keyword search** (FTS5) and **folder selection** (closure table). The design targets P95 search latency <500ms for 1M+ records with real-time filter updates.

---

## 1. Cross-Index Query Architecture

### 1.1 Query Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Interface                          │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐      │
│  │ Search Box   │    │ Folder Tree  │    │ Filters      │      │
│  │ "explosion"  │    │ SFX/Weapons  │    │ 48kHz, Stereo│      │
│  └──────┬───────┘    └──────┬───────┘    └──────┬───────┘      │
└─────────┼───────────────────┼───────────────────┼───────────────┘
          │                   │                   │
          ▼                   ▼                   ▼
┌─────────────────────────────────────────────────────────────────┐
│                    SearchEngine.search_with_folder()            │
│                                                                 │
│  1. Check cache (query + folder_id + filters → cache_key)      │
│  2. If miss: Build cross-index query                           │
│  3. Execute query with all parameters                          │
│  4. Cache results                                              │
│  5. Return (results, total_count)                              │
└─────────────────────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────────┐
│                        SQLite Database                          │
│                                                                 │
│  SELECT af.* FROM audio_files af                               │
│  INNER JOIN audio_search ON audio_search.rowid = af.id         │
│  INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id│
│  WHERE audio_search MATCH :query                               │
│    AND fc.ancestor_id = :folder_id                             │
│    AND af.sample_rate = :sample_rate                           │
│  ORDER BY bm25(audio_search)                                   │
│  LIMIT :limit OFFSET :offset                                   │
└─────────────────────────────────────────────────────────────────┘
```

### 1.2 Core Query Pattern

```sql
-- Cross-indexed search: keyword + folder + filters
SELECT
    af.id,
    af.file_path,
    af.filename,
    -bm25(audio_search) as score,
    af.sample_rate,
    af.duration,
    af.bit_depth,
    af.channels,
    af.file_size,
    af.user_tags,
    af.created_at,
    f.folder_path
FROM audio_files af
-- FTS5 keyword search
INNER JOIN audio_search ON audio_search.rowid = af.id
-- Folder hierarchy filter (via closure table)
INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
-- Folder name for display
LEFT JOIN folders f ON f.id = af.folder_id
WHERE
    -- Keyword search (FTS5 MATCH)
    audio_search MATCH :query
    -- Folder filter (includes all subfolders via closure table)
    AND fc.ancestor_id = :folder_id
    -- Additional filters (all optional)
    AND (:sample_rate IS NULL OR af.sample_rate = :sample_rate)
    AND (:min_duration IS NULL OR af.duration >= :min_duration)
    AND (:max_duration IS NULL OR af.duration <= :max_duration)
    AND (:bit_depth IS NULL OR af.bit_depth = :bit_depth)
    AND (:channels IS NULL OR af.channels = :channels)
ORDER BY
    score DESC,
    af.filename ASC
LIMIT :limit
OFFSET :offset;
```

---

## 2. SearchEngine Implementation

### 2.1 Extended SearchEngine Class

```python
# src/search/search_engine.py

from dataclasses import dataclass
from typing import List, Tuple, Optional, Dict, Any
from datetime import datetime, timedelta
import hashlib
import logging

logger = logging.getLogger('audiolib.search')

@dataclass
class CrossIndexSearchResult:
    """Result from cross-indexed search"""
    id: int
    file_path: str
    filename: str
    score: float
    sample_rate: int
    duration: float
    bit_depth: int
    channels: int
    file_size: int
    user_tags: Optional[str]
    created_at: datetime
    folder_path: Optional[str]


class SearchEngine:
    """
    FTS5 search engine with cross-index folder filtering.

    Supports simultaneous keyword search + folder selection with caching.
    """

    def __init__(self, db_manager, cache_size_mb: int = 50, cache_ttl_seconds: int = 300):
        self.db_manager = db_manager
        self._cache: Dict[str, Tuple[List[CrossIndexSearchResult], int, datetime]] = {}
        self._cache_max_size = cache_size_mb * 1024 * 1024  # Convert to bytes
        self._cache_ttl = timedelta(seconds=cache_ttl_seconds)
        self._cache_hits = 0
        self._cache_misses = 0

    def search_with_folder(
        self,
        query: Optional[str] = None,
        folder_id: Optional[int] = None,
        include_subfolders: bool = True,
        filters: Optional[Dict[str, Any]] = None,
        limit: int = 100,
        offset: int = 0,
        order_by: str = 'rank'
    ) -> Tuple[List[CrossIndexSearchResult], int]:
        """
        Perform cross-indexed search with folder filtering.

        Args:
            query: Search query string (FTS5 syntax supported)
            folder_id: Folder to filter by (None = all folders)
            include_subfolders: If True, include files in subfolders
            filters: Additional filters (sample_rate, duration, etc.)
            limit: Maximum results to return
            offset: Pagination offset
            order_by: Sort order ('rank', 'filename', 'duration', 'created_at')

        Returns:
            Tuple of (results list, total count)
        """
        # Sanitize query
        if query:
            query = self._sanitize_query(query)

        # Generate cache key
        cache_key = self._generate_cache_key(
            query, folder_id, include_subfolders, filters, limit, offset, order_by
        )

        # Check cache
        cached = self._get_from_cache(cache_key)
        if cached:
            self._cache_hits += 1
            return cached

        self._cache_misses += 1

        # Build and execute query
        results, total = self._execute_cross_index_query(
            query, folder_id, include_subfolders, filters, limit, offset, order_by
        )

        # Cache results
        self._add_to_cache(cache_key, results, total)

        return results, total

    def _execute_cross_index_query(
        self,
        query: Optional[str],
        folder_id: Optional[int],
        include_subfolders: bool,
        filters: Optional[Dict[str, Any]],
        limit: int,
        offset: int,
        order_by: str
    ) -> Tuple[List[CrossIndexSearchResult], int]:
        """Execute the cross-indexed search query"""

        filters = filters or {}

        # Build WHERE clauses
        where_clauses = []
        params = {
            'limit': limit,
            'offset': offset,
            'sample_rate': filters.get('sample_rate'),
            'min_duration': filters.get('min_duration'),
            'max_duration': filters.get('max_duration'),
            'bit_depth': filters.get('bit_depth'),
            'channels': filters.get('channels'),
        }

        # Keyword search clause
        if query:
            where_clauses.append("audio_search MATCH :query")
            params['query'] = query

        # Folder filter clause
        if folder_id:
            params['folder_id'] = folder_id
            if include_subfolders:
                where_clauses.append("fc.ancestor_id = :folder_id")
            else:
                where_clauses.append("af.folder_id = :folder_id")

        # Additional filter clauses
        if filters.get('sample_rate'):
            where_clauses.append("af.sample_rate = :sample_rate")
        if filters.get('min_duration'):
            where_clauses.append("af.duration >= :min_duration")
        if filters.get('max_duration'):
            where_clauses.append("af.duration <= :max_duration")
        if filters.get('bit_depth'):
            where_clauses.append("af.bit_depth = :bit_depth")
        if filters.get('channels'):
            where_clauses.append("af.channels = :channels")

        # Build WHERE clause string
        where_sql = " AND ".join(where_clauses) if where_clauses else "1=1"

        # Build ORDER BY clause
        order_sql = self._build_order_clause(order_by, has_fts=bool(query))

        # Build main query
        if query:
            # FTS5 query with optional folder filter
            if folder_id and include_subfolders:
                sql = f"""
                    SELECT
                        af.id, af.file_path, af.filename,
                        -bm25(audio_search) as score,
                        af.sample_rate, af.duration, af.bit_depth,
                        af.channels, af.file_size, af.user_tags,
                        af.created_at, f.folder_path
                    FROM audio_files af
                    INNER JOIN audio_search ON audio_search.rowid = af.id
                    INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
                    LEFT JOIN folders f ON f.id = af.folder_id
                    WHERE {where_sql}
                    {order_sql}
                    LIMIT :limit OFFSET :offset
                """
            elif folder_id:
                sql = f"""
                    SELECT
                        af.id, af.file_path, af.filename,
                        -bm25(audio_search) as score,
                        af.sample_rate, af.duration, af.bit_depth,
                        af.channels, af.file_size, af.user_tags,
                        af.created_at, f.folder_path
                    FROM audio_files af
                    INNER JOIN audio_search ON audio_search.rowid = af.id
                    LEFT JOIN folders f ON f.id = af.folder_id
                    WHERE {where_sql}
                    {order_sql}
                    LIMIT :limit OFFSET :offset
                """
            else:
                sql = f"""
                    SELECT
                        af.id, af.file_path, af.filename,
                        -bm25(audio_search) as score,
                        af.sample_rate, af.duration, af.bit_depth,
                        af.channels, af.file_size, af.user_tags,
                        af.created_at, f.folder_path
                    FROM audio_files af
                    INNER JOIN audio_search ON audio_search.rowid = af.id
                    LEFT JOIN folders f ON f.id = af.folder_id
                    WHERE {where_sql}
                    {order_sql}
                    LIMIT :limit OFFSET :offset
                """
        else:
            # Non-FTS query (folder + filters only)
            if folder_id and include_subfolders:
                sql = f"""
                    SELECT
                        af.id, af.file_path, af.filename,
                        0 as score,
                        af.sample_rate, af.duration, af.bit_depth,
                        af.channels, af.file_size, af.user_tags,
                        af.created_at, f.folder_path
                    FROM audio_files af
                    INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
                    LEFT JOIN folders f ON f.id = af.folder_id
                    WHERE {where_sql}
                    {order_sql}
                    LIMIT :limit OFFSET :offset
                """
            else:
                sql = f"""
                    SELECT
                        af.id, af.file_path, af.filename,
                        0 as score,
                        af.sample_rate, af.duration, af.bit_depth,
                        af.channels, af.file_size, af.user_tags,
                        af.created_at, f.folder_path
                    FROM audio_files af
                    LEFT JOIN folders f ON f.id = af.folder_id
                    WHERE {where_sql}
                    {order_sql}
                    LIMIT :limit OFFSET :offset
                """

        # Execute main query
        with self.db_manager.get_session() as session:
            from sqlalchemy import text
            result = session.execute(text(sql), params)
            rows = result.fetchall()

            results = [
                CrossIndexSearchResult(
                    id=row[0],
                    file_path=row[1],
                    filename=row[2],
                    score=row[3],
                    sample_rate=row[4],
                    duration=row[5],
                    bit_depth=row[6],
                    channels=row[7],
                    file_size=row[8],
                    user_tags=row[9],
                    created_at=row[10],
                    folder_path=row[11]
                )
                for row in rows
            ]

            # Get total count (without LIMIT/OFFSET)
            total = self._get_total_count(
                session, query, folder_id, include_subfolders, filters
            )

        return results, total

    def _get_total_count(
        self,
        session,
        query: Optional[str],
        folder_id: Optional[int],
        include_subfolders: bool,
        filters: Dict[str, Any]
    ) -> int:
        """Get total count for pagination"""

        where_clauses = []
        params = {
            'sample_rate': filters.get('sample_rate'),
            'min_duration': filters.get('min_duration'),
            'max_duration': filters.get('max_duration'),
            'bit_depth': filters.get('bit_depth'),
            'channels': filters.get('channels'),
        }

        if query:
            where_clauses.append("audio_search MATCH :query")
            params['query'] = query

        if folder_id:
            params['folder_id'] = folder_id
            if include_subfolders:
                where_clauses.append("fc.ancestor_id = :folder_id")
            else:
                where_clauses.append("af.folder_id = :folder_id")

        if filters.get('sample_rate'):
            where_clauses.append("af.sample_rate = :sample_rate")
        if filters.get('min_duration'):
            where_clauses.append("af.duration >= :min_duration")
        if filters.get('max_duration'):
            where_clauses.append("af.duration <= :max_duration")
        if filters.get('bit_depth'):
            where_clauses.append("af.bit_depth = :bit_depth")
        if filters.get('channels'):
            where_clauses.append("af.channels = :channels")

        where_sql = " AND ".join(where_clauses) if where_clauses else "1=1"

        if query and folder_id and include_subfolders:
            sql = f"""
                SELECT COUNT(*)
                FROM audio_files af
                INNER JOIN audio_search ON audio_search.rowid = af.id
                INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
                WHERE {where_sql}
            """
        elif query:
            sql = f"""
                SELECT COUNT(*)
                FROM audio_files af
                INNER JOIN audio_search ON audio_search.rowid = af.id
                WHERE {where_sql}
            """
        elif folder_id and include_subfolders:
            sql = f"""
                SELECT COUNT(*)
                FROM audio_files af
                INNER JOIN folder_closure fc ON fc.descendant_id = af.folder_id
                WHERE {where_sql}
            """
        else:
            sql = f"""
                SELECT COUNT(*) FROM audio_files af WHERE {where_sql}
            """

        from sqlalchemy import text
        result = session.execute(text(sql), params)
        return result.scalar() or 0

    def _build_order_clause(self, order_by: str, has_fts: bool) -> str:
        """Build ORDER BY clause"""
        orders = {
            'rank': 'score DESC, af.filename ASC' if has_fts else 'af.filename ASC',
            'filename': 'af.filename ASC',
            'filename_desc': 'af.filename DESC',
            'duration': 'af.duration ASC',
            'duration_desc': 'af.duration DESC',
            'created_at': 'af.created_at DESC',
            'sample_rate': 'af.sample_rate DESC',
        }
        return f"ORDER BY {orders.get(order_by, orders['rank'])}"

    def _sanitize_query(self, query: str) -> str:
        """Sanitize FTS5 query string"""
        # Remove dangerous characters
        query = query.replace(';', '').replace('--', '')

        # Limit length
        max_length = 500
        if len(query) > max_length:
            query = query[:max_length]

        # Escape special FTS5 characters for literal matching
        # (OR, AND, NOT, NEAR are preserved for advanced users)
        for char in ['"', "'", '(', ')', '*', '^', '{', '}', '[', ']']:
            query = query.replace(char, ' ')

        return query.strip()

    def _generate_cache_key(
        self,
        query: Optional[str],
        folder_id: Optional[int],
        include_subfolders: bool,
        filters: Optional[Dict],
        limit: int,
        offset: int,
        order_by: str
    ) -> str:
        """Generate unique cache key for query parameters"""
        key_parts = [
            query or '',
            str(folder_id or 0),
            str(include_subfolders),
            str(sorted(filters.items()) if filters else ''),
            str(limit),
            str(offset),
            order_by
        ]
        key_string = '|'.join(key_parts)
        return hashlib.md5(key_string.encode()).hexdigest()

    def _get_from_cache(self, key: str) -> Optional[Tuple[List[CrossIndexSearchResult], int]]:
        """Get results from cache if valid"""
        if key not in self._cache:
            return None

        results, total, timestamp = self._cache[key]
        if datetime.now() - timestamp > self._cache_ttl:
            del self._cache[key]
            return None

        return results, total

    def _add_to_cache(self, key: str, results: List[CrossIndexSearchResult], total: int):
        """Add results to cache with eviction if needed"""
        # Simple size-based eviction (remove oldest entries)
        while len(self._cache) > 1000:  # Max entries
            oldest_key = min(self._cache.keys(), key=lambda k: self._cache[k][2])
            del self._cache[oldest_key]

        self._cache[key] = (results, total, datetime.now())

    def invalidate_cache(self):
        """Invalidate entire cache (call on database changes)"""
        self._cache.clear()
        logger.debug("Search cache invalidated")

    def get_cache_stats(self) -> Dict[str, Any]:
        """Get cache statistics"""
        total = self._cache_hits + self._cache_misses
        return {
            'hits': self._cache_hits,
            'misses': self._cache_misses,
            'hit_rate': self._cache_hits / total if total > 0 else 0,
            'entries': len(self._cache),
        }
```

---

## 3. Folder Tree Queries

### 3.1 Get Child Folders with Filtered Counts

```python
def get_child_folders_with_counts(
    session,
    parent_id: Optional[int],
    query: Optional[str] = None,
    filters: Optional[Dict] = None
) -> List[Dict]:
    """
    Get child folders with file counts filtered by current search criteria.

    Args:
        session: Database session
        parent_id: Parent folder ID (None for root folders)
        query: Current search query
        filters: Current filter criteria

    Returns:
        List of folder dicts with filtered_count
    """
    from sqlalchemy import text

    # Build filtered files CTE
    filter_conditions = []
    params = {'parent_id': parent_id}

    if query:
        filter_conditions.append("audio_search MATCH :query")
        params['query'] = query

    if filters:
        if filters.get('sample_rate'):
            filter_conditions.append("af.sample_rate = :sample_rate")
            params['sample_rate'] = filters['sample_rate']
        if filters.get('min_duration'):
            filter_conditions.append("af.duration >= :min_duration")
            params['min_duration'] = filters['min_duration']
        # ... add other filters

    filter_where = " AND ".join(filter_conditions) if filter_conditions else "1=1"

    if query:
        # With FTS5 join
        sql = f"""
            WITH FilteredFiles AS (
                SELECT af.id, af.folder_id
                FROM audio_files af
                INNER JOIN audio_search ON audio_search.rowid = af.id
                WHERE {filter_where}
            )
            SELECT
                f.id,
                f.folder_name,
                f.folder_path,
                f.depth,
                f.total_file_count as total_count,
                (
                    SELECT COUNT(DISTINCT ff.id)
                    FROM FilteredFiles ff
                    INNER JOIN folder_closure fc ON fc.descendant_id = ff.folder_id
                    WHERE fc.ancestor_id = f.id
                ) as filtered_count,
                EXISTS(SELECT 1 FROM folders child WHERE child.parent_id = f.id) as has_children
            FROM folders f
            WHERE f.parent_id {'= :parent_id' if parent_id else 'IS NULL'}
            ORDER BY f.folder_name
        """
    else:
        # Without FTS5
        if filter_conditions:
            sql = f"""
                WITH FilteredFiles AS (
                    SELECT af.id, af.folder_id
                    FROM audio_files af
                    WHERE {filter_where}
                )
                SELECT
                    f.id,
                    f.folder_name,
                    f.folder_path,
                    f.depth,
                    f.total_file_count as total_count,
                    (
                        SELECT COUNT(DISTINCT ff.id)
                        FROM FilteredFiles ff
                        INNER JOIN folder_closure fc ON fc.descendant_id = ff.folder_id
                        WHERE fc.ancestor_id = f.id
                    ) as filtered_count,
                    EXISTS(SELECT 1 FROM folders child WHERE child.parent_id = f.id) as has_children
                FROM folders f
                WHERE f.parent_id {'= :parent_id' if parent_id else 'IS NULL'}
                ORDER BY f.folder_name
            """
        else:
            # No filters - use cached total_file_count
            sql = f"""
                SELECT
                    f.id,
                    f.folder_name,
                    f.folder_path,
                    f.depth,
                    f.total_file_count as total_count,
                    f.total_file_count as filtered_count,
                    EXISTS(SELECT 1 FROM folders child WHERE child.parent_id = f.id) as has_children
                FROM folders f
                WHERE f.parent_id {'= :parent_id' if parent_id else 'IS NULL'}
                ORDER BY f.folder_name
            """

    result = session.execute(text(sql), params)
    return [
        {
            'id': row[0],
            'name': row[1],
            'path': row[2],
            'depth': row[3],
            'total_count': row[4],
            'filtered_count': row[5],
            'has_children': bool(row[6])
        }
        for row in result.fetchall()
    ]
```

### 3.2 Get Folder Breadcrumb

```python
def get_folder_breadcrumb(session, folder_id: int) -> List[Dict]:
    """
    Get breadcrumb path from root to specified folder.

    Returns:
        List of folder dicts from root to current folder
    """
    from sqlalchemy import text

    sql = """
        SELECT f.id, f.folder_name, f.folder_path, fc.depth
        FROM folders f
        INNER JOIN folder_closure fc ON fc.ancestor_id = f.id
        WHERE fc.descendant_id = :folder_id
        ORDER BY fc.depth DESC
    """

    result = session.execute(text(sql), {'folder_id': folder_id})
    return [
        {'id': row[0], 'name': row[1], 'path': row[2], 'depth': row[3]}
        for row in result.fetchall()
    ]
```

---

## 4. Caching Strategy

### 4.1 Multi-Level Cache Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    L1: UI State Cache                       │
│  - Current search results (in memory)                       │
│  - Current folder tree state                                │
│  - Invalidation: On any user action                         │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                  L2: Search Results Cache                   │
│  - Query + folder_id + filters → results                   │
│  - TTL: 5 minutes                                           │
│  - Max entries: 1000                                        │
│  - Invalidation: On file add/delete/modify                  │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                L3: Folder Statistics Cache                  │
│  - folder_id + filter_hash → file_count                    │
│  - TTL: 5 minutes                                           │
│  - Invalidation: On file add/delete/move                    │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│            L4: Database (Denormalized Stats)                │
│  - folders.total_file_count (pre-computed)                 │
│  - folders.total_duration                                   │
│  - Update: On scan completion                               │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 Folder Statistics Cache Implementation

```python
# src/search/folder_cache.py

from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Dict, Optional, Tuple
import threading
import hashlib

@dataclass
class FolderCacheEntry:
    """Cached folder statistics"""
    folder_id: int
    filter_hash: str
    file_count: int
    timestamp: datetime


class FolderStatisticsCache:
    """
    In-memory cache for folder file counts.

    Caches filtered file counts per folder, invalidated when:
    1. Files added/deleted/moved
    2. Search/filter criteria change
    3. TTL expires (5 minutes default)
    """

    def __init__(self, ttl_seconds: int = 300, max_entries: int = 5000):
        self._cache: Dict[Tuple[int, str], FolderCacheEntry] = {}
        self._lock = threading.RLock()
        self._ttl = timedelta(seconds=ttl_seconds)
        self._max_entries = max_entries

    def get(self, folder_id: int, filter_hash: str) -> Optional[int]:
        """
        Get cached file count for folder with specific filters.

        Args:
            folder_id: Folder ID
            filter_hash: Hash of current filter criteria

        Returns:
            File count if cached and valid, None otherwise
        """
        with self._lock:
            key = (folder_id, filter_hash)
            entry = self._cache.get(key)

            if entry is None:
                return None

            # Check TTL
            if datetime.now() - entry.timestamp > self._ttl:
                del self._cache[key]
                return None

            return entry.file_count

    def set(self, folder_id: int, filter_hash: str, file_count: int):
        """Cache folder file count"""
        with self._lock:
            # Evict oldest entries if at capacity
            if len(self._cache) >= self._max_entries:
                self._evict_oldest(self._max_entries // 10)

            key = (folder_id, filter_hash)
            self._cache[key] = FolderCacheEntry(
                folder_id=folder_id,
                filter_hash=filter_hash,
                file_count=file_count,
                timestamp=datetime.now()
            )

    def invalidate_folder(self, folder_id: int):
        """Invalidate all entries for a specific folder"""
        with self._lock:
            keys_to_remove = [k for k in self._cache if k[0] == folder_id]
            for key in keys_to_remove:
                del self._cache[key]

    def invalidate_all(self):
        """Invalidate entire cache"""
        with self._lock:
            self._cache.clear()

    def _evict_oldest(self, count: int):
        """Evict oldest entries"""
        sorted_keys = sorted(
            self._cache.keys(),
            key=lambda k: self._cache[k].timestamp
        )
        for key in sorted_keys[:count]:
            del self._cache[key]

    @staticmethod
    def generate_filter_hash(query: Optional[str], filters: Optional[Dict]) -> str:
        """Generate hash of filter criteria"""
        parts = [query or '']
        if filters:
            parts.append(str(sorted(filters.items())))
        return hashlib.md5('|'.join(parts).encode()).hexdigest()[:16]
```

### 4.3 Cache Invalidation Triggers

```python
# src/scanner/scanner_service.py

class ScannerService:
    def __init__(self, search_engine, folder_cache):
        self.search_engine = search_engine
        self.folder_cache = folder_cache

    def _on_file_added(self, file_path: str, folder_id: int):
        """Called when file is added during scan"""
        # Invalidate search cache
        self.search_engine.invalidate_cache()

        # Invalidate folder statistics for this folder and ancestors
        self._invalidate_folder_ancestors(folder_id)

    def _on_file_deleted(self, file_path: str, folder_id: int):
        """Called when file is deleted"""
        self.search_engine.invalidate_cache()
        self._invalidate_folder_ancestors(folder_id)

    def _on_file_moved(self, old_folder_id: int, new_folder_id: int):
        """Called when file is moved between folders"""
        self.search_engine.invalidate_cache()
        self._invalidate_folder_ancestors(old_folder_id)
        self._invalidate_folder_ancestors(new_folder_id)

    def _invalidate_folder_ancestors(self, folder_id: int):
        """Invalidate folder and all its ancestors"""
        with self.data_provider.get_session() as session:
            from sqlalchemy import text
            result = session.execute(
                text("SELECT ancestor_id FROM folder_closure WHERE descendant_id = :id"),
                {'id': folder_id}
            )
            for (ancestor_id,) in result.fetchall():
                self.folder_cache.invalidate_folder(ancestor_id)
```

---

## 5. Performance Optimization

### 5.1 Query Optimization Techniques

| Technique | Implementation | Benefit |
|-----------|---------------|---------|
| Composite indexes | `(folder_id, sample_rate)` | Faster filtered queries |
| Covering indexes | Include all SELECT columns | Avoid table lookups |
| CTE for filtered counts | `WITH FilteredFiles AS (...)` | Single pass for counts |
| BM25 ranking | FTS5 built-in | Relevance without overhead |
| Parameterized queries | `:param` placeholders | Prevent SQL injection, enable query plan reuse |

### 5.2 Debounced Search Updates

```python
# src/gui_qt/services/service_bridge.py

from PyQt6.QtCore import QTimer

class ServiceBridge(QObject):
    def __init__(self):
        self._search_debounce_timer = QTimer()
        self._search_debounce_timer.setSingleShot(True)
        self._search_debounce_timer.setInterval(300)  # 300ms debounce
        self._search_debounce_timer.timeout.connect(self._execute_debounced_search)
        self._pending_search_params = None

    def search_debounced(self, query: str, folder_id: int, filters: Dict):
        """Debounced search - delays execution until user stops typing"""
        self._pending_search_params = (query, folder_id, filters)
        self._search_debounce_timer.start()

    def _execute_debounced_search(self):
        """Execute the debounced search"""
        if self._pending_search_params:
            query, folder_id, filters = self._pending_search_params
            self._pending_search_params = None
            self._execute_search(query, folder_id, filters)
```

---

## 6. Performance Targets

| Metric | Target | Measurement |
|--------|--------|-------------|
| Cross-index search (P95) | <500ms | 1M records, keyword + folder filter |
| Folder tree expansion | <100ms | Expand folder with 100+ children |
| Filtered count update | <50ms | Update counts after search change |
| Cache hit ratio | >80% | After warm-up period |
| Memory usage | <50MB | Cache size limit |
