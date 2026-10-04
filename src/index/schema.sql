CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

-- path_key: Ordner-Schlüssel (endet auf '\'), eine Zeile pro gescannter Wurzel
CREATE TABLE roots (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    path_key TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    status TEXT NOT NULL,              -- running | complete | aborted
    error_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE dirs (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    path_raw BLOB,                     -- nur bei nicht darstellbarem UTF-16 (WTF-8)
    path_key TEXT NOT NULL UNIQUE,
    parent_key TEXT,
    depth INTEGER NOT NULL,
    mode TEXT NOT NULL,                -- full | summary
    attrs INTEGER NOT NULL,
    mtime INTEGER,
    is_link INTEGER NOT NULL,
    link_target TEXT,
    direct_entries INTEGER NOT NULL,
    summary_size INTEGER,
    summary_files INTEGER,
    generation INTEGER NOT NULL
);

CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    dir_key TEXT NOT NULL,
    path TEXT NOT NULL,
    path_raw BLOB,
    path_key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    ext TEXT,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    ctime INTEGER,
    attrs INTEGER NOT NULL,
    cloud_only INTEGER NOT NULL,
    is_link INTEGER NOT NULL,
    link_target TEXT,
    volume_serial INTEGER,
    file_index INTEGER,
    nlinks INTEGER,
    partial_hash BLOB,
    full_hash BLOB,
    hash_status TEXT,                  -- NULL | ok | locked | changed | error
    generation INTEGER NOT NULL
);

CREATE TABLE errors (
    id INTEGER PRIMARY KEY,
    root_key TEXT NOT NULL,
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    message TEXT NOT NULL,
    generation INTEGER NOT NULL
);

-- Schema v2: EXIF-Aufnahmedatum (lokale Sekunden seit Epoche; NULL = kein EXIF),
-- gültig bei gleicher Größe und mtime. Eigene Tabelle, damit Re-Scans sie nicht löschen.
CREATE TABLE exif_cache (
    path_key TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    taken INTEGER
);

CREATE INDEX files_size ON files(size) WHERE cloud_only = 0 AND is_link = 0;
CREATE INDEX files_dir ON files(dir_key);
CREATE INDEX files_hash ON files(full_hash) WHERE full_hash IS NOT NULL;
CREATE INDEX dirs_parent ON dirs(parent_key);
