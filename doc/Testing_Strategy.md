# AudioLib Explorer - Testing Strategy

> Current validation uses the Rust workspace. See [Rust Validation](Rust_Validation.md) for executed checks; Python-specific commands below are legacy references.

## Current Rust database regressions

Run `cargo test -p audiolib-core -p audiolib-db --all-targets --locked` for schema, migration, search and library administration fixtures. `library_management.rs` verifies consistent WAL backup, restore/rollback with reused readers, rejection of invalid inputs, FTS5/trigger repair, read-only source preservation, atomic batch/import edits, relinking and database-only removal. `scan_management.rs` covers Unicode full-scan deletion, removed counts, per-path errors and changed WAV valid bits. Desktop tests cover draft draining and generation invalidation across library changes. Validate the release CLI with `tools/validate-library-management.ps1`; its fixtures use isolated libraries and synthetic WAVs. Preserve the original library and real WAV hashes. See [operation semantics](Rust_Library_Management.md) and [actual verification](Rust_Validation.md).

**Version:** 1.0
**Date:** 2025-11-24
**Status:** Testing Specification

---

## Test Pyramid

```
      /\
     /  \  E2E Tests (5%)
    /────\
   /      \ Integration Tests (15%)
  /────────\
 /          \ Unit Tests (80%)
/────────────\
```

**Target Coverage:** 70%+ unit test coverage

---

## 1. Unit Tests

### 1.1 Test Framework
- **Framework:** pytest 7.4+
- **Coverage:** pytest-cov
- **Mocking:** pytest-mock, unittest.mock

### 1.2 Test Structure

```
tests/
├── unit/
│   ├── test_scanner.py
│   ├── test_search_engine.py
│   ├── test_metadata_extractor.py
│   ├── test_waveform_renderer.py
│   ├── test_validators.py
│   ├── test_query_sanitizer.py
│   └── test_i18n.py
├── integration/
│   ├── test_scan_workflow.py
│   ├── test_search_workflow.py
│   └── test_database.py
├── e2e/
│   └── test_main_workflow.py
├── performance/
│   └── test_search_performance.py
└── conftest.py  # Shared fixtures
```

### 1.3 Example Unit Tests

```python
# tests/unit/test_search_engine.py
import pytest
from src.search.search_engine import SearchEngine
from src.database.models import AudioFile

@pytest.fixture
def test_db_session():
    """Create test database session"""
    # Implementation
    pass

def test_search_basic_query(test_db_session):
    """Test basic keyword search"""
    engine = SearchEngine(test_db_session)
    results = engine.search("explosion")

    assert len(results) > 0
    assert all('explosion' in r.filename.lower() for r in results)

def test_search_with_filters(test_db_session):
    """Test search with sample rate filter"""
    engine = SearchEngine(test_db_session)
    results = engine.search("explosion", filters={'sample_rate': 48000})

    assert all(r.sample_rate == 48000 for r in results)

def test_search_performance(test_db_session):
    """Test search meets performance target"""
    import time
    engine = SearchEngine(test_db_session)

    start = time.time()
    results = engine.search("footstep")
    latency = (time.time() - start) * 1000

    assert latency < 500, f"Search latency {latency}ms exceeds 500ms target"
```

---

## 2. Security Tests

### 2.1 Input Validation Tests

```python
# tests/unit/test_validators.py
import pytest
from src.utils.validators import InputValidator, ValidationError

def test_sql_injection_prevention():
    """Test SQL injection attempts are blocked"""
    malicious_queries = [
        "'; DROP TABLE audio_files; --",
        "1' OR '1'='1",
        "'; DELETE FROM audio_files WHERE '1'='1",
    ]

    from src.search.query_sanitizer import QuerySanitizer

    for query in malicious_queries:
        with pytest.raises(ValidationError):
            QuerySanitizer.sanitize_fts5_query(query)

def test_path_traversal_prevention():
    """Test path traversal attempts are blocked"""
    malicious_paths = [
        "../../etc/passwd",
        "..\\..\\windows\\system32\\config\\sam",
        "~/../../etc/shadow",
    ]

    for path in malicious_paths:
        with pytest.raises(ValidationError):
            InputValidator.validate_file_path(path, allowed_roots=["/library"])
```

---

## 3. Performance Tests

### 3.1 Search Performance Benchmark

```python
# tests/performance/test_search_performance.py
import pytest
import time
import numpy as np
from src.search.search_engine import SearchEngine

@pytest.mark.performance
def test_search_latency_100k_records(benchmark_db_100k):
    """Benchmark search on 100K records"""
    engine = SearchEngine(benchmark_db_100k)

    queries = ['explosion', 'footstep', 'door', 'ambient']
    latencies = []

    for _ in range(100):
        query = np.random.choice(queries)
        start = time.time()
        results = engine.search(query)
        latency = (time.time() - start) * 1000
        latencies.append(latency)

    p50 = np.percentile(latencies, 50)
    p95 = np.percentile(latencies, 95)
    p99 = np.percentile(latencies, 99)

    print(f"\nSearch Performance (100K records):")
    print(f"  P50: {p50:.1f}ms")
    print(f"  P95: {p95:.1f}ms")
    print(f"  P99: {p99:.1f}ms")

    assert p95 < 500, f"P95 latency {p95:.1f}ms exceeds 500ms target"
```

---

## 4. Integration Tests

```python
# tests/integration/test_scan_workflow.py
def test_complete_scan_workflow(temp_audio_files):
    """Test end-to-end scan workflow"""
    scanner = ScannerService(session)

    # Scan directory
    stats = scanner.scan_directory(temp_audio_files)

    # Verify results
    assert stats['files_added'] == 10
    assert stats['files_failed'] == 0

    # Verify database
    count = session.query(AudioFile).count()
    assert count == 10

    # Verify FTS index
    results = session.execute(
        "SELECT * FROM audio_search WHERE audio_search MATCH 'test'"
    ).fetchall()
    assert len(results) > 0
```

---

## 5. Test Execution

### 5.1 Run All Tests
```bash
pytest tests/
```

### 5.2 Run with Coverage
```bash
pytest --cov=src --cov-report=html tests/
```

### 5.3 Run Performance Tests Only
```bash
pytest -m performance tests/performance/
```

---

**END OF TESTING STRATEGY**
# Rust implementation checks

The primary runtime is now the Rust workspace. Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --all-targets --locked`. ASIO feature builds use the verified SDK via `tools/prepare-asio.ps1`.

Rust acceptance covers legacy migration preservation, actual Chinese recall, FTS5 update/delete correctness, matching filter counts, incremental scanning, cancellation, inaccessible directory protection, stream decoding/resampling, transport state and bounded Waveform memory. Tests use temporary state and do not write the original WAV fixtures or SQLite library.

Release-level checks are `tools/validate-runtime.ps1` (migration/search/scanning), `tools/validate-playback-ipc.ps1` (confirmed desktop transport and muted bounded-memory playback) and `tools/validate-access-denial.ps1` (real Windows ACL denial in an owned temporary subtree, restored in finally). Run `cargo run --release --locked -p audiolib-db --bin search-benchmark -- --records 1000000 --queries 100` for the real provider benchmark. The program refuses to replace an existing library.

After packaging, `tools/validate-portable.ps1` checks the actual package binaries, first-launch import, Chinese UI snapshot, CLI statistics, license materials and fatal-startup logging. It uses isolated user data and leaves the original database unchanged.

`tools/validate-review-fixes.ps1` exercises the release CLI after the code-review fixes, including child-first directory import, deleted BWF fields, Unicode case folding, one-time legacy preferences and cross-process scan contention/release. Desktop regression tests additionally exercise selection/edit revisions and asynchronous close with pending saves. See [Review fixes](Rust_Review_Fixes.md).

The performance target is **P95 latency <500ms for 1M+ records** through the actual Rust DataProvider. Warm-cache numbers and prepared/device-open audio latency must be reported separately from uncached/cold measurements. Actual ASIO output routing and audible latency require hardware validation; device enumeration alone is not evidence of them. See [Rust Validation](Rust_Validation.md) for recorded results.

