# AudioLib Explorer - API Design Specification

**Version:** 1.0
**Date:** 2025-11-23
**Status:** Phase 2 Specification (Future Implementation)
**Related Documents:**
- Product Requirements: `AudioLib_Explorer_PRD.md`
- Technical Architecture: `Technical_Architecture.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [API Architecture](#2-api-architecture)
3. [Authentication & Authorization](#3-authentication--authorization)
4. [Core Endpoints](#4-core-endpoints)
5. [Request/Response Formats](#5-requestresponse-formats)
6. [Error Handling](#6-error-handling)
7. [Rate Limiting & Performance](#7-rate-limiting--performance)
8. [Versioning Strategy](#8-versioning-strategy)
9. [Migration from Phase 1](#9-migration-from-phase-1)

---

## 1. Overview

### 1.1 Purpose

This document specifies the REST API design for **AudioLib Explorer Phase 2** (client-server architecture). The API enables:

- Remote audio library access from multiple clients
- Centralized metadata management
- Collaborative tagging and annotations
- Server-side scanning and indexing
- Real-time search across distributed libraries

**Phase 1 Note:** This API is NOT implemented in Phase 1. Phase 1 uses `LocalDataProvider` with direct SQLite access. This specification serves as:
- Interface contract for future implementation
- Guide for maintaining client abstraction layer
- Documentation for Phase 2 planning

### 1.2 Design Principles

**1. RESTful Design**
- Resource-oriented URLs (`/files/{id}`, `/playlists/{id}`)
- Standard HTTP methods (GET, POST, PUT, DELETE)
- Stateless requests (all auth via tokens)

**2. JSON-First**
- All request/response bodies in JSON
- ISO 8601 timestamps
- UTF-8 encoding

**3. Backward Compatibility**
- API versioning via URL path (`/api/v1/...`)
- Deprecation warnings for old endpoints
- 6-month deprecation window

**4. Performance**
- Pagination for all list endpoints
- Partial response support (field filtering)
- Compression (gzip) for large responses

**5. Security**
- JWT-based authentication
- Role-based access control (RBAC)
- HTTPS required in production

### 1.3 Base URL

**Development:**
```
http://localhost:8000/api/v1
```

**Production:**
```
https://audiolib.example.com/api/v1
```

---

## 2. API Architecture

### 2.1 System Architecture Diagram

```
┌────────────────────────────────────────────────────────────┐
│                     Client Applications                     │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────────────┐  │
│  │ Desktop App  │  │ Web App      │  │ Mobile App      │  │
│  │ (tkinter)    │  │ (React)      │  │ (future)        │  │
│  └──────┬───────┘  └──────┬───────┘  └────────┬────────┘  │
└─────────┼──────────────────┼────────────────────┼──────────┘
          │                  │                    │
          └──────────────────┼────────────────────┘
                             │ HTTPS/JSON
┌────────────────────────────▼──────────────────────────────┐
│                      API Gateway                           │
│  ┌──────────────────────────────────────────────────────┐ │
│  │  - Authentication & Authorization                    │ │
│  │  - Rate Limiting                                     │ │
│  │  - Request Validation                                │ │
│  │  - Response Caching                                  │ │
│  └──────────────────────────────────────────────────────┘ │
└────────────────────────────┬──────────────────────────────┘
                             │
┌────────────────────────────▼──────────────────────────────┐
│                    Application Server                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐   │
│  │ Search      │  │ Scan        │  │ Metadata        │   │
│  │ Service     │  │ Service     │  │ Service         │   │
│  └─────────────┘  └─────────────┘  └─────────────────┘   │
│                                                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐   │
│  │ Waveform    │  │ User        │  │ Playlist        │   │
│  │ Service     │  │ Service     │  │ Service         │   │
│  └─────────────┘  └─────────────┘  └─────────────────┘   │
└────────────────────────────┬──────────────────────────────┘
                             │
┌────────────────────────────▼──────────────────────────────┐
│                      Data Layer                            │
│  ┌──────────────────┐           ┌──────────────────────┐  │
│  │ PostgreSQL       │           │ Elasticsearch        │  │
│  │ (Metadata)       │           │ (Full-Text Search)   │  │
│  └──────────────────┘           └──────────────────────┘  │
│                                                            │
│  ┌──────────────────┐           ┌──────────────────────┐  │
│  │ Redis            │           │ S3 / Object Storage  │  │
│  │ (Cache, Sessions)│           │ (Audio Files)        │  │
│  └──────────────────┘           └──────────────────────┘  │
└────────────────────────────────────────────────────────────┘
```

### 2.2 Technology Stack (Phase 2 Recommendations)

| Component | Technology | Rationale |
|-----------|-----------|-----------|
| **API Framework** | FastAPI (Python 3.11+) | Async support, auto API docs, type hints |
| **Database** | PostgreSQL 15+ | Production-grade, JSON support, FTS via pg_trgm |
| **Search Engine** | Elasticsearch 8+ | Industry standard for full-text search |
| **Cache** | Redis 7+ | Fast in-memory cache, session storage |
| **Object Storage** | MinIO / S3 | Scalable file storage, CDN integration |
| **Auth** | JWT + OAuth2 | Stateless, industry standard |
| **API Gateway** | Kong / nginx | Rate limiting, routing, SSL termination |
| **Monitoring** | Prometheus + Grafana | Metrics, alerting, dashboards |

---

## 3. Authentication & Authorization

### 3.1 Authentication Flow

**JWT (JSON Web Token) Authentication:**

1. **User Login:**
```http
POST /api/v1/auth/login
Content-Type: application/json

{
  "username": "john@example.com",
  "password": "SecurePassword123"
}

Response 200 OK:
{
  "access_token": "eyJhbGciOiJIUzI1NiIs...",
  "refresh_token": "eyJhbGciOiJIUzI1NiIs...",
  "token_type": "Bearer",
  "expires_in": 3600
}
```

2. **Authenticated Request:**
```http
GET /api/v1/files/search?q=explosion
Authorization: Bearer eyJhbGciOiJIUzI1NiIs...
```

3. **Token Refresh:**
```http
POST /api/v1/auth/refresh
Content-Type: application/json

{
  "refresh_token": "eyJhbGciOiJIUzI1NiIs..."
}

Response 200 OK:
{
  "access_token": "eyJhbGciOiJIUzI1NiIs...",
  "expires_in": 3600
}
```

### 3.2 Authorization (RBAC)

**Roles:**

| Role | Permissions | Use Case |
|------|-------------|----------|
| **Admin** | Full access, user management, system config | Studio manager |
| **Editor** | Read, write, delete files; manage playlists | Sound designers |
| **Viewer** | Read-only access, search, listen | Clients, reviewers |
| **Scanner** | Scan directories, upload metadata (no read) | Automated scanners |

**Permission Matrix:**

| Operation | Admin | Editor | Viewer | Scanner |
|-----------|-------|--------|--------|---------|
| Search files | ✓ | ✓ | ✓ | ✗ |
| View metadata | ✓ | ✓ | ✓ | ✗ |
| Edit tags/notes | ✓ | ✓ | ✗ | ✗ |
| Delete files | ✓ | ✓ | ✗ | ✗ |
| Scan directories | ✓ | ✓ | ✗ | ✓ |
| Manage users | ✓ | ✗ | ✗ | ✗ |
| System config | ✓ | ✗ | ✗ | ✗ |

**JWT Payload:**
```json
{
  "sub": "user123",
  "email": "john@example.com",
  "role": "editor",
  "permissions": ["read", "write", "delete"],
  "iat": 1700000000,
  "exp": 1700003600
}
```

### 3.3 API Key Authentication (Alternative)

For programmatic access (scripts, integrations):

```http
GET /api/v1/files/search?q=explosion
X-API-Key: ak_live_1234567890abcdef
```

**API Key Scopes:**
- `read:files` - Read file metadata
- `write:files` - Modify file metadata
- `scan:directories` - Initiate directory scans
- `admin:*` - Full administrative access

---

## 4. Core Endpoints

### 4.1 File Search & Discovery

#### 4.1.1 Search Files

**Endpoint:** `GET /api/v1/files/search`

**Description:** Full-text search across audio files with filtering and pagination.

**Query Parameters:**

| Parameter | Type | Required | Description | Example |
|-----------|------|----------|-------------|---------|
| `q` | string | Yes | Search query (FTS syntax) | `explosion OR blast` |
| `sample_rate` | integer | No | Filter by sample rate (Hz) | `48000` |
| `bit_depth` | integer | No | Filter by bit depth | `24` |
| `channels` | integer | No | Filter by channel count | `2` |
| `min_duration` | float | No | Minimum duration (seconds) | `1.0` |
| `max_duration` | float | No | Maximum duration (seconds) | `10.0` |
| `is_favorite` | boolean | No | Favorites only | `true` |
| `rating_min` | integer | No | Minimum rating (1-5) | `4` |
| `tags` | string | No | Comma-separated tags | `outdoor,weapon` |
| `limit` | integer | No | Results per page (max 1000) | `100` |
| `offset` | integer | No | Pagination offset | `0` |
| `sort` | string | No | Sort field | `duration`, `created_date` |
| `order` | string | No | Sort order | `asc`, `desc` |

**Request Example:**
```http
GET /api/v1/files/search?q=footstep&sample_rate=48000&limit=50&offset=0
Authorization: Bearer eyJhbGciOiJIUzI1NiIs...
```

**Response 200 OK:**
```json
{
  "total": 234,
  "limit": 50,
  "offset": 0,
  "results": [
    {
      "id": 12345,
      "filename": "footstep_concrete_01.wav",
      "file_path": "/library/sfx/footsteps/concrete/footstep_concrete_01.wav",
      "file_size": 524288,
      "file_hash": "sha256:abcdef1234567890...",
      "sample_rate": 48000,
      "bit_depth": 24,
      "channels": 2,
      "duration": 2.5,
      "created_date": "2024-01-15T10:30:00Z",
      "modified_date": "2024-01-15T10:30:00Z",
      "scanned_date": "2024-11-23T08:00:00Z",
      "bext_description": "Footstep on concrete surface, male, medium pace",
      "bext_originator": "SFX Library Pro",
      "ixml_project": "Urban Ambience",
      "ixml_scene": "Street",
      "ixml_take": "Take 3",
      "user_rating": 4,
      "is_favorite": false,
      "user_tags": ["footstep", "concrete", "outdoor"],
      "user_notes": "Good for urban scenes",
      "peak_amplitude": 0.85,
      "rms_level": 0.32,
      "spectral_centroid": 1250.5
    },
    {
      "id": 12346,
      "filename": "footstep_gravel_05.wav",
      // ... more fields
    }
  ],
  "facets": {
    "sample_rate": {
      "44100": 50,
      "48000": 150,
      "96000": 34
    },
    "bit_depth": {
      "16": 80,
      "24": 154
    },
    "channels": {
      "1": 100,
      "2": 134
    }
  }
}
```

**Response 400 Bad Request:**
```json
{
  "error": {
    "code": "INVALID_PARAMETER",
    "message": "Invalid sample_rate: must be positive integer",
    "field": "sample_rate"
  }
}
```

#### 4.1.2 Get File by ID

**Endpoint:** `GET /api/v1/files/{file_id}`

**Description:** Retrieve single audio file metadata by ID.

**Path Parameters:**
- `file_id` (integer, required) - Audio file ID

**Request Example:**
```http
GET /api/v1/files/12345
Authorization: Bearer eyJhbGciOiJIUzI1NiIs...
```

**Response 200 OK:**
```json
{
  "id": 12345,
  "filename": "footstep_concrete_01.wav",
  // ... all metadata fields (same as search result)
}
```

**Response 404 Not Found:**
```json
{
  "error": {
    "code": "FILE_NOT_FOUND",
    "message": "Audio file with ID 12345 not found"
  }
}
```

#### 4.1.3 Get File Download URL

**Endpoint:** `GET /api/v1/files/{file_id}/download`

**Description:** Get presigned URL for downloading audio file.

**Response 200 OK:**
```json
{
  "url": "https://cdn.audiolib.com/files/12345/footstep_concrete_01.wav?token=...",
  "expires_at": "2024-11-23T09:00:00Z",
  "file_size": 524288
}
```

### 4.2 Metadata Management

#### 4.2.1 Update File Metadata

**Endpoint:** `PATCH /api/v1/files/{file_id}`

**Description:** Update user-editable metadata fields (tags, notes, rating, favorite).

**Request Body:**
```json
{
  "user_rating": 5,
  "is_favorite": true,
  "user_tags": ["footstep", "concrete", "outdoor", "urban"],
  "user_notes": "Perfect for urban street scenes. Clear, punchy sound."
}
```

**Response 200 OK:**
```json
{
  "id": 12345,
  "updated_fields": ["user_rating", "is_favorite", "user_tags", "user_notes"],
  "updated_at": "2024-11-23T08:15:00Z"
}
```

**Response 403 Forbidden:**
```json
{
  "error": {
    "code": "PERMISSION_DENIED",
    "message": "User role 'viewer' cannot modify file metadata"
  }
}
```

#### 4.2.2 Batch Update Metadata

**Endpoint:** `POST /api/v1/files/batch-update`

**Description:** Update metadata for multiple files in single request.

**Request Body:**
```json
{
  "file_ids": [12345, 12346, 12347],
  "updates": {
    "user_tags": ["footstep", "outdoor"],
    "user_rating": 4
  }
}
```

**Response 200 OK:**
```json
{
  "updated_count": 3,
  "failed_count": 0,
  "updated_ids": [12345, 12346, 12347]
}
```

### 4.3 Scanning & Indexing

#### 4.3.1 Initiate Directory Scan

**Endpoint:** `POST /api/v1/scan`

**Description:** Start server-side directory scan for audio files.

**Request Body:**
```json
{
  "directory_path": "/mnt/audio-library/sfx",
  "recursive": true,
  "file_pattern": "*.wav",
  "overwrite_existing": false
}
```

**Response 202 Accepted:**
```json
{
  "scan_id": "scan_abc123",
  "status": "pending",
  "directory_path": "/mnt/audio-library/sfx",
  "created_at": "2024-11-23T08:00:00Z",
  "status_url": "/api/v1/scan/scan_abc123"
}
```

#### 4.3.2 Get Scan Status

**Endpoint:** `GET /api/v1/scan/{scan_id}`

**Description:** Check status of ongoing or completed scan.

**Response 200 OK (In Progress):**
```json
{
  "scan_id": "scan_abc123",
  "status": "running",
  "progress": {
    "current": 5000,
    "total": 10000,
    "percentage": 50
  },
  "statistics": {
    "files_scanned": 5000,
    "files_added": 4500,
    "files_updated": 300,
    "files_failed": 200
  },
  "started_at": "2024-11-23T08:00:00Z",
  "estimated_completion": "2024-11-23T09:00:00Z"
}
```

**Response 200 OK (Completed):**
```json
{
  "scan_id": "scan_abc123",
  "status": "completed",
  "statistics": {
    "files_scanned": 10000,
    "files_added": 9200,
    "files_updated": 500,
    "files_failed": 300
  },
  "started_at": "2024-11-23T08:00:00Z",
  "completed_at": "2024-11-23T09:15:00Z",
  "duration_seconds": 4500
}
```

#### 4.3.3 Cancel Scan

**Endpoint:** `DELETE /api/v1/scan/{scan_id}`

**Description:** Cancel running scan operation.

**Response 200 OK:**
```json
{
  "scan_id": "scan_abc123",
  "status": "cancelled",
  "statistics": {
    "files_scanned": 3000,
    "files_added": 2700,
    "files_updated": 200,
    "files_failed": 100
  }
}
```

### 4.4 Playlists

#### 4.4.1 List Playlists

**Endpoint:** `GET /api/v1/playlists`

**Query Parameters:**
- `limit` (integer, default 100)
- `offset` (integer, default 0)

**Response 200 OK:**
```json
{
  "total": 15,
  "limit": 100,
  "offset": 0,
  "playlists": [
    {
      "id": 1,
      "name": "Explosion Sound Effects",
      "description": "All explosion sounds for action scenes",
      "file_count": 42,
      "total_duration": 125.5,
      "created_date": "2024-10-01T10:00:00Z",
      "modified_date": "2024-11-20T15:30:00Z"
    },
    // ... more playlists
  ]
}
```

#### 4.4.2 Create Playlist

**Endpoint:** `POST /api/v1/playlists`

**Request Body:**
```json
{
  "name": "Urban Ambience",
  "description": "City background sounds and atmospheres",
  "file_ids": [12345, 12346, 12347]
}
```

**Response 201 Created:**
```json
{
  "id": 16,
  "name": "Urban Ambience",
  "description": "City background sounds and atmospheres",
  "file_count": 3,
  "created_date": "2024-11-23T08:30:00Z"
}
```

#### 4.4.3 Add Files to Playlist

**Endpoint:** `POST /api/v1/playlists/{playlist_id}/files`

**Request Body:**
```json
{
  "file_ids": [12348, 12349]
}
```

**Response 200 OK:**
```json
{
  "playlist_id": 16,
  "added_count": 2,
  "total_file_count": 5
}
```

#### 4.4.4 Remove Files from Playlist

**Endpoint:** `DELETE /api/v1/playlists/{playlist_id}/files`

**Request Body:**
```json
{
  "file_ids": [12345]
}
```

**Response 200 OK:**
```json
{
  "playlist_id": 16,
  "removed_count": 1,
  "total_file_count": 4
}
```

### 4.5 Waveform Cache

#### 4.5.1 Get Waveform

**Endpoint:** `GET /api/v1/files/{file_id}/waveform`

**Query Parameters:**
- `zoom_level` (string, enum: `overview`, `minute`, `second`) - Default: `overview`

**Response 200 OK:**
```json
{
  "file_id": 12345,
  "zoom_level": "overview",
  "waveform_data_url": "data:application/octet-stream;base64,iVBORw0KGgo...",
  "samples_per_pixel": 1000,
  "generated_at": "2024-11-23T08:00:00Z",
  "cache_hit": true
}
```

**Response 202 Accepted (Cache Miss - Generating):**
```json
{
  "file_id": 12345,
  "zoom_level": "overview",
  "status": "generating",
  "estimated_completion": "2024-11-23T08:01:00Z",
  "retry_after": 5
}
```

### 4.6 Statistics & Reporting

#### 4.6.1 Get Database Statistics

**Endpoint:** `GET /api/v1/stats`

**Response 200 OK:**
```json
{
  "total_files": 125432,
  "total_size_bytes": 523456789012,
  "total_duration_seconds": 345678.5,
  "sample_rate_distribution": {
    "44100": 25000,
    "48000": 80000,
    "96000": 15000,
    "192000": 5432
  },
  "bit_depth_distribution": {
    "16": 30000,
    "24": 90000,
    "32": 5432
  },
  "channels_distribution": {
    "1": 60000,
    "2": 55000,
    "6": 8000,
    "8": 2432
  },
  "last_scan": "2024-11-23T08:00:00Z",
  "database_size_mb": 2048
}
```

---

## 5. Request/Response Formats

### 5.1 Common Data Types

#### AudioFile Object

```json
{
  "id": 12345,
  "filename": "footstep_concrete_01.wav",
  "file_path": "/library/sfx/footsteps/concrete/footstep_concrete_01.wav",
  "file_size": 524288,
  "file_hash": "sha256:abcdef1234567890...",

  "sample_rate": 48000,
  "bit_depth": 24,
  "channels": 2,
  "duration": 2.5,

  "bext_description": "Footstep on concrete surface, male, medium pace",
  "bext_originator": "SFX Library Pro",
  "bext_originator_reference": "PROJ-2024-001",
  "bext_originator_date": "2024-01-15",
  "bext_originator_time": "10:30:00",
  "bext_time_reference": 0,
  "bext_coding_history": "A=PCM,F=48000,W=24,M=stereo",

  "ixml_project": "Urban Ambience",
  "ixml_scene": "Street",
  "ixml_take": "Take 3",
  "ixml_tape": "Card 01",
  "ixml_file_uid": "12345-ABCD-6789",

  "peak_amplitude": 0.85,
  "rms_level": 0.32,
  "spectral_centroid": 1250.5,
  "zero_crossing_rate": 0.15,

  "created_date": "2024-01-15T10:30:00Z",
  "modified_date": "2024-01-15T10:30:00Z",
  "scanned_date": "2024-11-23T08:00:00Z",
  "last_accessed": "2024-11-23T09:15:00Z",

  "user_rating": 4,
  "is_favorite": false,
  "user_tags": ["footstep", "concrete", "outdoor"],
  "user_notes": "Good for urban scenes",
  "project_references": [
    "/projects/urban-game/session.rpp",
    "/projects/film-sfx/edit-01.ptx"
  ],

  "import_source": "SFX Library Pro v3"
}
```

#### Playlist Object

```json
{
  "id": 1,
  "name": "Explosion Sound Effects",
  "description": "All explosion sounds for action scenes",
  "file_count": 42,
  "total_duration": 125.5,
  "created_date": "2024-10-01T10:00:00Z",
  "modified_date": "2024-11-20T15:30:00Z",
  "items": [
    {
      "position": 1,
      "file_id": 12345,
      "file": { /* AudioFile object */ }
    },
    // ... more items
  ]
}
```

#### ScanSession Object

```json
{
  "scan_id": "scan_abc123",
  "scan_path": "/mnt/audio-library/sfx",
  "scan_type": "full",
  "status": "completed",

  "statistics": {
    "files_scanned": 10000,
    "files_added": 9200,
    "files_updated": 500,
    "files_failed": 300
  },

  "started_at": "2024-11-23T08:00:00Z",
  "completed_at": "2024-11-23T09:15:00Z",
  "duration_seconds": 4500,

  "error_message": null
}
```

### 5.2 Pagination Format

All list endpoints use consistent pagination:

**Request:**
```http
GET /api/v1/files/search?q=explosion&limit=50&offset=100
```

**Response:**
```json
{
  "total": 234,
  "limit": 50,
  "offset": 100,
  "results": [ /* array of items */ ],
  "next": "/api/v1/files/search?q=explosion&limit=50&offset=150",
  "previous": "/api/v1/files/search?q=explosion&limit=50&offset=50"
}
```

### 5.3 Filtering and Sorting

**Standard Query Parameters:**
- `limit` - Results per page (default 100, max 1000)
- `offset` - Pagination offset (default 0)
- `sort` - Sort field (e.g., `created_date`, `duration`, `filename`)
- `order` - Sort order (`asc` or `desc`, default `asc`)
- `fields` - Comma-separated list of fields to include (partial response)

**Example - Partial Response:**
```http
GET /api/v1/files/search?q=explosion&fields=id,filename,duration
```

```json
{
  "total": 234,
  "results": [
    {
      "id": 12345,
      "filename": "explosion_large_01.wav",
      "duration": 5.2
    },
    // ... only requested fields
  ]
}
```

---

## 6. Error Handling

### 6.1 Error Response Format

**Standard Error Object:**

```json
{
  "error": {
    "code": "ERROR_CODE",
    "message": "Human-readable error message",
    "field": "parameter_name",  // Optional: which field caused error
    "details": { /* Optional: additional context */ }
  }
}
```

### 6.2 HTTP Status Codes

| Status Code | Meaning | When to Use |
|-------------|---------|-------------|
| **200 OK** | Success | Successful GET, PATCH, DELETE |
| **201 Created** | Resource created | Successful POST creating new resource |
| **202 Accepted** | Async operation started | Scan initiated, waveform generating |
| **204 No Content** | Success with no body | Successful DELETE with no response |
| **400 Bad Request** | Invalid request | Malformed JSON, invalid parameters |
| **401 Unauthorized** | Not authenticated | Missing or invalid JWT token |
| **403 Forbidden** | Not authorized | Valid token but insufficient permissions |
| **404 Not Found** | Resource not found | File ID doesn't exist |
| **409 Conflict** | Resource conflict | Duplicate filename, concurrent update |
| **422 Unprocessable Entity** | Validation failed | Valid JSON but business logic error |
| **429 Too Many Requests** | Rate limit exceeded | Client exceeded rate limit |
| **500 Internal Server Error** | Server error | Unexpected server failure |
| **503 Service Unavailable** | Service down | Database offline, maintenance mode |

### 6.3 Error Codes

**Authentication Errors (4xx):**
- `AUTH_TOKEN_MISSING` - No Authorization header
- `AUTH_TOKEN_INVALID` - Malformed or expired JWT
- `AUTH_TOKEN_EXPIRED` - Token expired (use refresh token)
- `PERMISSION_DENIED` - Insufficient permissions for operation

**Validation Errors (4xx):**
- `INVALID_PARAMETER` - Parameter failed validation
- `MISSING_PARAMETER` - Required parameter not provided
- `INVALID_JSON` - Request body is not valid JSON

**Resource Errors (4xx):**
- `FILE_NOT_FOUND` - Audio file ID doesn't exist
- `PLAYLIST_NOT_FOUND` - Playlist ID doesn't exist
- `SCAN_NOT_FOUND` - Scan ID doesn't exist
- `DUPLICATE_FILE` - File already exists in database

**Server Errors (5xx):**
- `INTERNAL_ERROR` - Unexpected server error
- `DATABASE_ERROR` - Database connection failed
- `SEARCH_ENGINE_ERROR` - Elasticsearch unavailable
- `STORAGE_ERROR` - File storage unavailable

**Example Error Responses:**

**400 Bad Request:**
```json
{
  "error": {
    "code": "INVALID_PARAMETER",
    "message": "Invalid sample_rate: must be positive integer",
    "field": "sample_rate",
    "details": {
      "provided_value": "-1",
      "allowed_values": [44100, 48000, 96000, 192000]
    }
  }
}
```

**401 Unauthorized:**
```json
{
  "error": {
    "code": "AUTH_TOKEN_EXPIRED",
    "message": "JWT token has expired. Use refresh token to obtain new access token.",
    "details": {
      "expired_at": "2024-11-23T08:00:00Z",
      "refresh_endpoint": "/api/v1/auth/refresh"
    }
  }
}
```

**403 Forbidden:**
```json
{
  "error": {
    "code": "PERMISSION_DENIED",
    "message": "User role 'viewer' cannot modify file metadata",
    "details": {
      "user_role": "viewer",
      "required_permission": "write:files"
    }
  }
}
```

**429 Too Many Requests:**
```json
{
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "Rate limit exceeded: 100 requests per minute",
    "details": {
      "limit": 100,
      "window_seconds": 60,
      "retry_after": 45
    }
  }
}
```

---

## 7. Rate Limiting & Performance

### 7.1 Rate Limits

**Per-User Limits:**

| Endpoint Category | Rate Limit | Window |
|------------------|------------|--------|
| Search (`/search`) | 100 requests/min | 60 seconds |
| Metadata reads (`GET /files/*`) | 500 requests/min | 60 seconds |
| Metadata writes (`PATCH /files/*`) | 50 requests/min | 60 seconds |
| Scan operations (`POST /scan`) | 5 requests/hour | 3600 seconds |
| Authentication (`/auth/*`) | 10 requests/min | 60 seconds |

**Rate Limit Headers:**

```http
HTTP/1.1 200 OK
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 45
X-RateLimit-Reset: 1700003600
```

**Rate Limit Exceeded Response:**

```http
HTTP/1.1 429 Too Many Requests
Retry-After: 45
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1700003600

{
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "Rate limit exceeded: 100 requests per minute",
    "details": {
      "retry_after": 45
    }
  }
}
```

### 7.2 Response Compression

**Request Compression:**
```http
GET /api/v1/files/search?q=explosion
Accept-Encoding: gzip, deflate
```

**Response Compression:**
```http
HTTP/1.1 200 OK
Content-Encoding: gzip
Content-Length: 5432
```

**Note:** Responses >1KB are automatically compressed.

### 7.3 Caching

**Cache-Control Headers:**

```http
# Read-only endpoints (file metadata)
Cache-Control: public, max-age=300  # 5 minutes

# User-specific data (playlists, annotations)
Cache-Control: private, max-age=60  # 1 minute

# Dynamic data (search results)
Cache-Control: no-cache

# Static assets (waveforms)
Cache-Control: public, max-age=86400  # 24 hours
```

**ETag Support:**

```http
# Request
GET /api/v1/files/12345
If-None-Match: "abc123"

# Response (not modified)
HTTP/1.1 304 Not Modified
ETag: "abc123"

# Response (modified)
HTTP/1.1 200 OK
ETag: "def456"
{ /* updated file data */ }
```

---

## 8. Versioning Strategy

### 8.1 URL-Based Versioning

**Current Version:**
```
/api/v1/files/search
```

**Future Version:**
```
/api/v2/files/search
```

### 8.2 Deprecation Policy

1. **Announcement:** 6 months before deprecation
2. **Deprecation Header:** Added to old endpoints
   ```http
   HTTP/1.1 200 OK
   Deprecation: true
   Sunset: Sat, 01 Jun 2025 00:00:00 GMT
   Link: </api/v2/files/search>; rel="successor-version"
   ```

3. **Migration Period:** 6 months with both versions running
4. **Sunset:** Old version removed after migration period

### 8.3 Backward Compatibility

**Guaranteed:**
- Adding new optional fields to responses (non-breaking)
- Adding new endpoints (non-breaking)
- Adding new optional query parameters (non-breaking)

**Breaking Changes (require new version):**
- Removing fields from responses
- Changing field types
- Removing endpoints
- Changing authentication method

---

## 9. Migration from Phase 1

### 9.1 Database Export/Import

#### 9.1.1 Export Local Database

**Desktop App Function:**

```python
# src/database/export.py

def export_database_to_json(db_path: str, output_path: str):
    """Export local SQLite database to JSON for server import"""
    session = create_session(db_path)

    export_data = {
        "version": "1.0",
        "exported_at": datetime.now().isoformat(),
        "total_files": session.query(AudioFile).count(),
        "files": []
    }

    for audio_file in session.query(AudioFile).all():
        export_data["files"].append({
            "filename": audio_file.filename,
            "file_path": audio_file.file_path,
            # ... all fields
        })

    with open(output_path, 'w', encoding='utf-8') as f:
        json.dump(export_data, f, indent=2)
```

#### 9.1.2 Import to Server

**Endpoint:** `POST /api/v1/import`

**Request:**
```http
POST /api/v1/import
Content-Type: application/json
Authorization: Bearer eyJhbGciOiJIUzI1NiIs...

{
  "version": "1.0",
  "exported_at": "2024-11-23T08:00:00Z",
  "total_files": 10000,
  "files": [ /* array of AudioFile objects */ ]
}
```

**Response 202 Accepted:**
```json
{
  "import_id": "import_xyz789",
  "status": "processing",
  "total_files": 10000,
  "status_url": "/api/v1/import/import_xyz789"
}
```

### 9.2 Client Configuration

**Phase 1 Config (local mode):**
```json
{
  "mode": "local",
  "database_path": "./database/default.db"
}
```

**Phase 2 Config (remote mode):**
```json
{
  "mode": "remote",
  "server_url": "https://audiolib.example.com/api/v1",
  "auth_token": "eyJhbGciOiJIUzI1NiIs...",
  "cache_enabled": true,
  "cache_ttl_seconds": 300
}
```

**Desktop App Automatically Switches:**
```python
# src/main.py

config = get_config()

if config.mode == 'local':
    data_provider = LocalDataProvider(session)
elif config.mode == 'remote':
    data_provider = RemoteDataProvider(config.server_url, config.auth_token)
```

---

**END OF API DESIGN SPECIFICATION**
