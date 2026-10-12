# AudioLib Explorer - Security Guidelines

**Version:** 1.0
**Date:** 2025-11-24
**Status:** Security Specification
**Related Documents:**
- Technical Architecture: `Technical_Architecture.md`
- API Design: `API_Design.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Input Validation](#2-input-validation)
3. [SQL Injection Prevention](#3-sql-injection-prevention)
4. [File System Security](#4-file-system-security)
5. [Data Privacy](#5-data-privacy)
6. [Error Handling Security](#6-error-handling-security)
7. [Security Checklist](#7-security-checklist)

---

## 1. Overview

### 1.1 Security Principles

**Defense in Depth:**
- Multiple layers of security controls
- Input validation at every boundary
- Fail securely (deny by default)

**Principle of Least Privilege:**
- File system access limited to necessary directories
- Database permissions restricted
- Plugin sandboxing

**Security by Design:**
- Security considerations from Day 1
- No security through obscurity
- Regular security reviews

### 1.2 Threat Model

**Primary Threats:**
1. **SQL Injection** via FTS5 search queries
2. **Path Traversal** via file path inputs
3. **Arbitrary Code Execution** via malicious plugins
4. **Data Exposure** via error messages or logs

**Out of Scope (Phase 1):**
- Network-based attacks (no network component in Phase 1)
- Authentication bypass (no authentication in Phase 1)
- DDoS attacks (local application)

---

## 2. Input Validation

### 2.1 Validation Framework

**Validation Strategy:**
```python
# src/utils/validators.py

import os
import re
from pathlib import Path
from typing import Union, List
from src.utils.logger import get_logger

logger = get_logger(__name__)

class ValidationError(Exception):
    """Custom exception for validation failures"""
    pass

class InputValidator:
    """Centralized input validation for all user inputs"""

    # Allowed file extensions
    ALLOWED_AUDIO_EXTENSIONS = {'.wav', '.WAV'}

    # Maximum lengths
    MAX_SEARCH_QUERY_LENGTH = 1000
    MAX_FILE_PATH_LENGTH = 4096
    MAX_TAG_LENGTH = 100
    MAX_TAGS_COUNT = 50
    MAX_NOTE_LENGTH = 10000

    # Forbidden patterns
    FORBIDDEN_SQL_KEYWORDS = {
        'DROP', 'DELETE', 'INSERT', 'UPDATE', 'ALTER',
        'CREATE', 'EXEC', 'EXECUTE', 'SCRIPT'
    }

    FORBIDDEN_PATH_PATTERNS = [
        '..',           # Parent directory traversal
        '~',            # Home directory expansion
        '$',            # Environment variable expansion
        '\\\\',         # UNC paths (\\server\share)
    ]
```

### 2.2 Search Query Validation

**FTS5 Query Sanitization:**

```python
# src/search/query_sanitizer.py

import re
from src.utils.validators import InputValidator, ValidationError

class QuerySanitizer:
    """Sanitize FTS5 search queries to prevent injection"""

    @staticmethod
    def sanitize_fts5_query(query: str) -> str:
        """
        Sanitize user search query for FTS5

        Args:
            query: Raw user input search query

        Returns:
            Sanitized query safe for FTS5 MATCH

        Raises:
            ValidationError: If query contains forbidden content
        """
        if not query or not isinstance(query, str):
            raise ValidationError("Query must be non-empty string")

        # 1. Length check
        if len(query) > InputValidator.MAX_SEARCH_QUERY_LENGTH:
            raise ValidationError(
                f"Query too long (max {InputValidator.MAX_SEARCH_QUERY_LENGTH} chars)"
            )

        # 2. Remove SQL comments
        query = re.sub(r'--.*$', '', query, flags=re.MULTILINE)
        query = re.sub(r'/\*.*?\*/', '', query, flags=re.DOTALL)

        # 3. Check for forbidden SQL keywords
        query_upper = query.upper()
        for keyword in InputValidator.FORBIDDEN_SQL_KEYWORDS:
            if keyword in query_upper:
                raise ValidationError(
                    f"Forbidden SQL keyword in query: {keyword}"
                )

        # 4. Escape special characters
        # FTS5 allows: *, ", AND, OR, NOT, NEAR, but we validate their usage

        # 5. Remove semicolons (statement separator)
        if ';' in query:
            raise ValidationError("Semicolons not allowed in search queries")

        # 6. Strip whitespace
        query = query.strip()

        logger.debug(f"Sanitized query: {query}")
        return query

    @staticmethod
    def validate_fts5_syntax(query: str) -> bool:
        """
        Validate FTS5 query syntax without executing

        Returns:
            True if syntax is valid, False otherwise
        """
        # Check balanced quotes
        if query.count('"') % 2 != 0:
            raise ValidationError("Unbalanced quotes in query")

        # Check balanced parentheses
        if query.count('(') != query.count(')'):
            raise ValidationError("Unbalanced parentheses in query")

        # Validate operators
        operators = ['AND', 'OR', 'NOT', 'NEAR']
        tokens = query.upper().split()

        # Operators should not be at start/end
        if tokens and tokens[0] in operators:
            raise ValidationError(f"Query cannot start with operator: {tokens[0]}")
        if tokens and tokens[-1] in operators:
            raise ValidationError(f"Query cannot end with operator: {tokens[-1]}")

        return True

# Usage in SearchEngine
from src.search.query_sanitizer import QuerySanitizer

class SearchEngine:
    def search(self, query: str, filters: dict = None):
        # Sanitize query
        safe_query = QuerySanitizer.sanitize_fts5_query(query)

        # Validate syntax
        QuerySanitizer.validate_fts5_syntax(safe_query)

        # Execute search with parameterized query
        sql = """
            SELECT audio_files.*
            FROM audio_files
            JOIN audio_search ON audio_files.id = audio_search.rowid
            WHERE audio_search MATCH :query
            ORDER BY rank
            LIMIT :limit
        """
        results = self.session.execute(sql, {'query': safe_query, 'limit': 1000})
        return results.fetchall()
```

### 2.3 File Path Validation

**Path Traversal Prevention:**

```python
# src/utils/validators.py (continued)

class InputValidator:

    @staticmethod
    def validate_file_path(path: str, allowed_roots: List[str] = None) -> Path:
        """
        Validate file path to prevent directory traversal attacks

        Args:
            path: User-provided file path
            allowed_roots: List of allowed root directories
                          If None, no root restriction

        Returns:
            Validated Path object (absolute, normalized)

        Raises:
            ValidationError: If path is invalid or forbidden
        """
        if not path or not isinstance(path, str):
            raise ValidationError("Path must be non-empty string")

        # 1. Length check
        if len(path) > InputValidator.MAX_FILE_PATH_LENGTH:
            raise ValidationError(
                f"Path too long (max {InputValidator.MAX_FILE_PATH_LENGTH} chars)"
            )

        # 2. Check for forbidden patterns
        for pattern in InputValidator.FORBIDDEN_PATH_PATTERNS:
            if pattern in path:
                raise ValidationError(
                    f"Forbidden pattern in path: {pattern}"
                )

        # 3. Convert to absolute path (resolves '..' and symlinks)
        try:
            abs_path = Path(path).resolve(strict=False)
        except Exception as e:
            raise ValidationError(f"Invalid path: {e}")

        # 4. Check if file exists (optional, depends on use case)
        # For scan operations, we allow non-existent directories
        # For file reads, we require existence

        # 5. Check if path is within allowed roots
        if allowed_roots:
            is_allowed = any(
                str(abs_path).startswith(str(Path(root).resolve()))
                for root in allowed_roots
            )

            if not is_allowed:
                raise ValidationError(
                    f"Path outside allowed directories: {abs_path}"
                )

        logger.debug(f"Validated path: {abs_path}")
        return abs_path

    @staticmethod
    def validate_audio_file_path(path: str) -> Path:
        """
        Validate audio file path with extension check

        Args:
            path: Path to audio file

        Returns:
            Validated Path object

        Raises:
            ValidationError: If not a valid WAV file path
        """
        abs_path = InputValidator.validate_file_path(path)

        # Check file extension
        if abs_path.suffix not in InputValidator.ALLOWED_AUDIO_EXTENSIONS:
            raise ValidationError(
                f"Invalid audio file extension: {abs_path.suffix}. "
                f"Allowed: {InputValidator.ALLOWED_AUDIO_EXTENSIONS}"
            )

        # Check if file exists and is readable
        if not abs_path.exists():
            raise ValidationError(f"File not found: {abs_path}")

        if not abs_path.is_file():
            raise ValidationError(f"Path is not a file: {abs_path}")

        return abs_path
```

### 2.4 User Annotation Validation

**Tags, Notes, Rating Validation:**

```python
# src/utils/validators.py (continued)

class InputValidator:

    @staticmethod
    def validate_tags(tags: Union[List[str], str]) -> List[str]:
        """
        Validate user tags

        Args:
            tags: List of tag strings or comma-separated string

        Returns:
            List of validated tag strings

        Raises:
            ValidationError: If tags are invalid
        """
        # Convert comma-separated string to list
        if isinstance(tags, str):
            tags = [t.strip() for t in tags.split(',')]

        if not isinstance(tags, list):
            raise ValidationError("Tags must be list or comma-separated string")

        # Check tag count
        if len(tags) > InputValidator.MAX_TAGS_COUNT:
            raise ValidationError(
                f"Too many tags (max {InputValidator.MAX_TAGS_COUNT})"
            )

        # Validate each tag
        validated_tags = []
        for tag in tags:
            if not isinstance(tag, str):
                raise ValidationError(f"Tag must be string: {tag}")

            tag = tag.strip()

            # Skip empty tags
            if not tag:
                continue

            # Length check
            if len(tag) > InputValidator.MAX_TAG_LENGTH:
                raise ValidationError(
                    f"Tag too long: {tag} (max {InputValidator.MAX_TAG_LENGTH} chars)"
                )

            # Alphanumeric + spaces + hyphens only
            if not re.match(r'^[\w\s\-]+$', tag, re.UNICODE):
                raise ValidationError(
                    f"Tag contains invalid characters: {tag}"
                )

            validated_tags.append(tag)

        return validated_tags

    @staticmethod
    def validate_notes(notes: str) -> str:
        """
        Validate user notes

        Args:
            notes: User-provided notes text

        Returns:
            Validated notes string

        Raises:
            ValidationError: If notes are invalid
        """
        if not isinstance(notes, str):
            raise ValidationError("Notes must be string")

        # Length check
        if len(notes) > InputValidator.MAX_NOTE_LENGTH:
            raise ValidationError(
                f"Notes too long (max {InputValidator.MAX_NOTE_LENGTH} chars)"
            )

        # Strip leading/trailing whitespace
        notes = notes.strip()

        # No validation on content (allow any Unicode text)
        # Could add XSS prevention if notes are rendered as HTML (future)

        return notes

    @staticmethod
    def validate_rating(rating: int) -> int:
        """
        Validate user rating

        Args:
            rating: Rating value (1-5 stars)

        Returns:
            Validated rating integer

        Raises:
            ValidationError: If rating is invalid
        """
        if not isinstance(rating, int):
            raise ValidationError("Rating must be integer")

        if not 1 <= rating <= 5:
            raise ValidationError("Rating must be between 1 and 5")

        return rating
```

---

## 3. SQL Injection Prevention

### 3.1 Parameterized Queries (Always)

**Rule:** NEVER use string concatenation or f-strings for SQL queries

**Good (Safe):**
```python
# Use parameterized queries with :param syntax
sql = "SELECT * FROM audio_files WHERE id = :id"
result = session.execute(sql, {'id': file_id})
```

**Bad (Vulnerable):**
```python
# NEVER DO THIS - SQL Injection vulnerability
sql = f"SELECT * FROM audio_files WHERE id = {file_id}"
result = session.execute(sql)
```

### 3.2 SQLAlchemy ORM Usage

**Prefer ORM over Raw SQL:**

```python
# Good: ORM query (injection-safe)
audio_file = session.query(AudioFile).filter(
    AudioFile.filename == user_input
).first()

# Also good: Parameterized raw SQL
sql = "SELECT * FROM audio_files WHERE filename = :filename"
result = session.execute(sql, {'filename': user_input})

# Bad: String concatenation
sql = f"SELECT * FROM audio_files WHERE filename = '{user_input}'"
result = session.execute(sql)  # VULNERABLE!
```

### 3.3 FTS5 Query Safety

**Always use sanitized queries:**

```python
from src.search.query_sanitizer import QuerySanitizer

# In SearchEngine.search()
def search(self, query: str, filters: dict = None):
    # Step 1: Sanitize
    safe_query = QuerySanitizer.sanitize_fts5_query(query)

    # Step 2: Validate syntax
    QuerySanitizer.validate_fts5_syntax(safe_query)

    # Step 3: Use parameterized query
    sql = """
        SELECT audio_files.*
        FROM audio_files
        JOIN audio_search ON audio_files.id = audio_search.rowid
        WHERE audio_search MATCH :query
        ORDER BY rank
    """

    results = self.session.execute(sql, {'query': safe_query})
    return results.fetchall()
```

---

## 4. File System Security

### 4.1 Safe File Reading

**Always validate paths before file operations:**

```python
# src/scanner/metadata_extractor.py

from src.utils.validators import InputValidator, ValidationError
import soundfile as sf

def extract_audio_metadata(file_path: str) -> dict:
    """
    Extract metadata from audio file with path validation

    Args:
        file_path: Path to audio file

    Returns:
        Dictionary of metadata

    Raises:
        ValidationError: If path is invalid
    """
    try:
        # Validate path
        safe_path = InputValidator.validate_audio_file_path(file_path)

        # Read audio file
        audio_data, sample_rate = sf.read(str(safe_path))

        # Extract metadata
        metadata = {
            'sample_rate': sample_rate,
            'channels': audio_data.shape[1] if len(audio_data.shape) > 1 else 1,
            'duration': len(audio_data) / sample_rate,
            # ... more metadata
        }

        return metadata

    except ValidationError as e:
        logger.error(f"Invalid file path: {e}")
        raise
    except Exception as e:
        logger.error(f"Failed to extract metadata from {file_path}: {e}")
        raise
```

### 4.2 Directory Scanning Security

**Restrict scanning to allowed directories:**

```python
# src/scanner/scanner_service.py

from src.utils.validators import InputValidator

class ScannerService:
    def __init__(self, session, allowed_scan_roots: List[str] = None):
        self.session = session
        # Default allowed roots (configurable)
        self.allowed_scan_roots = allowed_scan_roots or [
            str(Path.home() / "Music"),
            str(Path.home() / "AudioLibrary"),
            "D:\\AudioSamples",  # Example: dedicated audio drive
        ]

    def scan_directory(self, directory_path: str, recursive: bool = True):
        """
        Scan directory for audio files with security checks

        Args:
            directory_path: Root directory to scan
            recursive: Include subdirectories
        """
        try:
            # Validate directory path
            safe_path = InputValidator.validate_file_path(
                directory_path,
                allowed_roots=self.allowed_scan_roots
            )

            # Check if path exists and is directory
            if not safe_path.exists():
                raise ValidationError(f"Directory not found: {safe_path}")

            if not safe_path.is_dir():
                raise ValidationError(f"Path is not a directory: {safe_path}")

            # Scan files
            self._scan_directory_recursive(safe_path, recursive)

        except ValidationError as e:
            logger.error(f"Scan directory validation failed: {e}")
            raise

    def _scan_directory_recursive(self, path: Path, recursive: bool):
        """Internal scan implementation"""
        # Implementation...
        pass
```

### 4.3 Temporary File Handling

**Secure temporary file operations:**

```python
import tempfile
import os
from pathlib import Path

def generate_waveform_with_temp_file(audio_file: AudioFile):
    """Generate waveform using secure temporary file"""

    # Use tempfile module (creates files in OS temp dir with secure permissions)
    with tempfile.NamedTemporaryFile(
        mode='wb',
        suffix='.png',
        delete=True,  # Auto-delete on close
        prefix='audiolib_waveform_'
    ) as temp_file:

        # Generate waveform
        waveform_image = render_waveform(audio_file)

        # Save to temp file
        waveform_image.save(temp_file.name)

        # Process temp file
        # ...

        # File automatically deleted when leaving 'with' block
```

---

## 5. Data Privacy

### 5.1 No Telemetry by Default

**Opt-in analytics only:**

```python
# src/utils/config.py

class PrivacyConfig(BaseModel):
    telemetry_enabled: bool = False  # Default: OFF
    crash_reporting_enabled: bool = False  # Default: OFF
    share_anonymous_usage: bool = False  # Default: OFF

# In GUI: Explicit opt-in dialog on first run
class FirstRunDialog:
    def show(self):
        """Show first-run privacy consent dialog"""
        message = t('privacy.telemetry_consent_message')
        # User must explicitly check boxes to enable
        # No pre-checked boxes (dark patterns)
```

### 5.2 Local Data Storage Only

**Phase 1: No cloud synchronization:**

- All data stored locally in SQLite
- No network requests (except web search for docs, if user initiated)
- No automatic updates without user consent

### 5.3 Sensitive Data Handling

**Do not log sensitive information:**

```python
# Good: Log sanitized data
logger.info(f"Searching for query: {query[:20]}...")  # Truncate

# Bad: Log full user input (may contain private search terms)
logger.info(f"Searching for query: {query}")  # Could leak sensitive info
```

---

## 6. Error Handling Security

### 6.1 Safe Error Messages

**Do not expose system details in error messages:**

```python
# Good: User-friendly error without system details
try:
    audio_file = read_audio_file(path)
except Exception as e:
    logger.error(f"Failed to read audio file: {path}", exc_info=True)  # Full details in log
    show_error_dialog(
        "Cannot read audio file",
        "The file may be corrupted or in an unsupported format."
    )  # Generic message to user

# Bad: Expose file paths and internal errors to user
try:
    audio_file = read_audio_file(path)
except Exception as e:
    show_error_dialog(
        "Error",
        f"Failed to read {path}: {str(e)}"  # Exposes internal details!
    )
```

### 6.2 Exception Handling Best Practices

**Catch specific exceptions:**

```python
# Good: Specific exception handling
try:
    validate_file_path(user_path)
except ValidationError as e:
    # Expected validation error
    logger.warning(f"Validation failed: {e}")
    show_error_dialog("Invalid Input", str(e))
except PermissionError as e:
    # File permission issue
    logger.error(f"Permission denied: {user_path}")
    show_error_dialog("Permission Denied", "Cannot access file")
except Exception as e:
    # Unexpected error
    logger.exception("Unexpected error during path validation")
    show_error_dialog("Error", "An unexpected error occurred")

# Bad: Catch-all exception hides security issues
try:
    validate_file_path(user_path)
except Exception:
    pass  # Silently ignoring errors - BAD!
```

---

## 7. Security Checklist

### 7.1 Development Checklist

**Before Committing Code:**

- [ ] All user inputs validated using `InputValidator`
- [ ] All SQL queries use parameterized queries (no f-strings)
- [ ] All file paths validated before file operations
- [ ] No hardcoded credentials or API keys
- [ ] No sensitive data in log messages
- [ ] Error messages are user-friendly (no system details)
- [ ] Exceptions are caught specifically (not generic `Exception`)

### 7.2 Code Review Checklist

**Security-Focused Review:**

- [ ] Check for SQL injection vulnerabilities (search for f-strings in SQL)
- [ ] Check for path traversal vulnerabilities (search for `os.path.join` with user input)
- [ ] Check for hardcoded secrets (search for 'password', 'api_key', 'token')
- [ ] Check for insecure file permissions (search for `chmod`, `os.chmod`)
- [ ] Check for unvalidated user input (search for `request.get`, `input()`)
- [ ] Check for unsafe deserialization (search for `pickle.loads`, `eval`)

### 7.3 Testing Checklist

**Security Test Cases:**

- [ ] Test SQL injection in search query (should be sanitized)
- [ ] Test path traversal in file path (should be rejected)
- [ ] Test overly long inputs (should be rejected or truncated)
- [ ] Test special characters in tags/notes (should be handled safely)
- [ ] Test null/empty inputs (should be rejected gracefully)
- [ ] Test malformed file paths (should be rejected)

### 7.4 Deployment Checklist

**Before Release:**

- [ ] All security guidelines implemented
- [ ] Security test suite passing
- [ ] No known security vulnerabilities in dependencies (check with `safety`)
- [ ] Code signing certificate applied (Windows)
- [ ] Privacy policy created (if telemetry enabled)
- [ ] Security contact email published

---

## Appendix A: Common Vulnerabilities

### A.1 SQL Injection Examples

**Vulnerable Code:**
```python
# VULNERABLE
query = f"SELECT * FROM audio_files WHERE filename LIKE '%{user_input}%'"
```

**Attack:**
```python
user_input = "'; DROP TABLE audio_files; --"
# Result: SELECT * FROM audio_files WHERE filename LIKE '%'; DROP TABLE audio_files; --%'
```

**Fix:**
```python
query = "SELECT * FROM audio_files WHERE filename LIKE :pattern"
session.execute(query, {'pattern': f'%{user_input}%'})
```

### A.2 Path Traversal Examples

**Vulnerable Code:**
```python
# VULNERABLE
file_path = os.path.join(base_dir, user_input)
with open(file_path, 'r') as f:
    content = f.read()
```

**Attack:**
```python
user_input = "../../etc/passwd"
# Result: Opens /etc/passwd instead of intended file
```

**Fix:**
```python
safe_path = InputValidator.validate_file_path(
    user_input,
    allowed_roots=[base_dir]
)
with open(safe_path, 'r') as f:
    content = f.read()
```

---

**END OF SECURITY GUIDELINES**
