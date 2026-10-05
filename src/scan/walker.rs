use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use crossbeam_channel::Sender;

use super::classify::{Classifier, DirMode};
use super::source::{DirSource, EntryKind, RawEntry};
use crate::index::{DirRecord, FileRecord, PrevFile, ScanErrorRecord};
use crate::paths;

pub enum ScanEvent {
    Dir(DirRecord),
    Files(Vec<FileRecord>),
    Error(ScanErrorRecord),
}

/// Zähler für die Fortschrittsanzeige.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub current: Mutex<String>,
}

pub struct WalkCtx<'a> {
    pub source: &'a dyn DirSource,
    pub classifier: &'a Classifier,
    pub cancel: &'a AtomicBool,
    pub progress: &'a Progress,
    /// Vorzustand aus dem Index; unveränderte Dateien behalten ihre Hashes.
    pub prev: &'a HashMap<String, PrevFile>,
    pub tx: &'a Sender<ScanEvent>,
    /// `first_seen` für neue Schlüssel setzen (Wurzel war schon einmal vollständig gescannt).
    pub first_seen: Option<i64>,
}

pub fn error_kind(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::PermissionDenied => "access_denied",
        io::ErrorKind::NotFound => "vanished",
        _ => "io",
    }
}

impl WalkCtx<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn send(&self, event: ScanEvent) {
        // Ein geschlossener Kanal bedeutet: der Writer ist ausgefallen, der Fehler kommt dort.
        let _ = self.tx.send(event);
    }

    fn error(&self, path: &Path, e: &io::Error) {
        self.progress.errors.fetch_add(1, Ordering::Relaxed);
        self.send(ScanEvent::Error(ScanErrorRecord {
            path: paths::display(path),
            kind: error_kind(e).to_string(),
            message: e.to_string(),
        }));
    }
}

/// Läuft den Baum unterhalb von `root` parallel ab (im aktuellen rayon-Pool).
pub fn walk(ctx: &WalkCtx<'_>, root: &Path) {
    rayon::scope(|s| visit(s, ctx, root.to_path_buf(), 0, 0, None));
}

fn has_project_file(entries: &[RawEntry]) -> bool {
    entries.iter().any(|e| {
        let name = e.name.to_string_lossy().to_lowercase();
        name.ends_with(".csproj") || name.ends_with(".sln")
    })
}

fn visit<'s>(
    scope: &rayon::Scope<'s>,
    ctx: &'s WalkCtx<'s>,
    path: PathBuf,
    depth: i64,
    attrs: u32,
    mtime: Option<i64>,
) {
    if ctx.cancelled() {
        return;
    }
    *ctx.progress.current.lock().unwrap() = paths::display(&path);
    ctx.progress.dirs.fetch_add(1, Ordering::Relaxed);

    let dir_key = paths::dir_key(&path);
    let mut record = DirRecord {
        path: paths::display(&path),
        path_key: dir_key.clone(),
        parent_key: (depth > 0)
            .then(|| path.parent().map(paths::dir_key))
            .flatten(),
        depth,
        mode: "full".into(),
        attrs,
        mtime,
        ..DirRecord::default()
    };

    let listing = match ctx.source.read_dir(&path) {
        Ok(l) => l,
        Err(e) => {
            ctx.error(&path, &e);
            ctx.send(ScanEvent::Dir(record));
            return;
        }
    };
    for (name, e) in &listing.errors {
        ctx.error(&path.join(name), e);
    }
    record.direct_entries = (listing.entries.len() + listing.errors.len()) as i64;
    let parent_has_project = has_project_file(&listing.entries);

    let mut files = Vec::new();
    for entry in &listing.entries {
        let child = path.join(&entry.name);
        match entry.kind {
            EntryKind::File => {
                if ctx.classifier.is_excluded_file(&child) {
                    continue;
                }
                files.push(file_record(ctx, &dir_key, &child, entry, false));
            }
            EntryKind::Link { dir: false } => {
                files.push(file_record(ctx, &dir_key, &child, entry, true))
            }
            EntryKind::Link { dir: true } => ctx.send(ScanEvent::Dir(DirRecord {
                path: paths::display(&child),
                path_key: paths::dir_key(&child),
                parent_key: Some(dir_key.clone()),
                depth: depth + 1,
                mode: "full".into(),
                attrs: entry.attrs.0,
                mtime: Some(entry.mtime),
                is_link: true,
                link_target: entry.link_target.clone(),
                ..DirRecord::default()
            })),
            EntryKind::Dir => match ctx.classifier.classify_dir(&child, parent_has_project) {
                DirMode::Exclude => {}
                DirMode::SummaryOnly => {
                    let summary = summarize(ctx, &child);
                    ctx.send(ScanEvent::Dir(DirRecord {
                        path: paths::display(&child),
                        path_key: paths::dir_key(&child),
                        parent_key: Some(dir_key.clone()),
                        depth: depth + 1,
                        mode: "summary".into(),
                        attrs: entry.attrs.0,
                        mtime: Some(entry.mtime),
                        direct_entries: summary.direct,
                        summary_size: Some(summary.size as i64),
                        summary_files: Some(summary.files as i64),
                        ..DirRecord::default()
                    }));
                }
                DirMode::Full => {
                    let (a, m) = (entry.attrs.0, Some(entry.mtime));
                    scope.spawn(move |s| visit(s, ctx, child, depth + 1, a, m));
                }
            },
        }
    }

    if !files.is_empty() {
        let bytes: u64 = files.iter().map(|f| f.size as u64).sum();
        ctx.progress
            .files
            .fetch_add(files.len() as u64, Ordering::Relaxed);
        ctx.progress.bytes.fetch_add(bytes, Ordering::Relaxed);
        ctx.send(ScanEvent::Files(files));
    }
    ctx.send(ScanEvent::Dir(record));
}

fn file_record(
    ctx: &WalkCtx<'_>,
    dir_key: &str,
    path: &Path,
    entry: &RawEntry,
    is_link: bool,
) -> FileRecord {
    let path_key = paths::path_key(path);
    let first_seen = ctx.first_seen.filter(|_| !ctx.prev.contains_key(&path_key));
    let size = if is_link { 0 } else { entry.size as i64 };
    // Hashes nur übernehmen, wenn Größe und Änderungsdatum gleich sind und der letzte
    // Versuch nicht fehlgeschlagen ist (gesperrte Dateien werden erneut versucht).
    let carried = ctx.prev.get(&path_key).filter(|p| {
        p.size == size
            && p.mtime == entry.mtime
            && matches!(p.hash_status.as_deref(), None | Some("ok"))
    });
    let name = entry.name.to_string_lossy().into_owned();
    FileRecord {
        dir_key: dir_key.to_string(),
        path: paths::display(path),
        path_key,
        ext: Path::new(&name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase()),
        name,
        size,
        mtime: entry.mtime,
        ctime: Some(entry.ctime),
        first_seen,
        attrs: entry.attrs.0,
        cloud_only: !is_link && entry.attrs.is_cloud_only(),
        is_link,
        link_target: entry.link_target.clone(),
        partial_hash: carried.and_then(|p| p.partial_hash.clone()),
        full_hash: carried.and_then(|p| p.full_hash.clone()),
        hash_status: carried.and_then(|p| p.hash_status.clone()),
    }
}

struct Summary {
    size: u64,
    files: u64,
    direct: i64,
}

/// Summiert einen Teilbaum nur auf (keine Einzeleinträge); Links werden nicht betreten.
fn summarize(ctx: &WalkCtx<'_>, root: &Path) -> Summary {
    let mut summary = Summary {
        size: 0,
        files: 0,
        direct: 0,
    };
    let mut stack = vec![(root.to_path_buf(), true)];
    while let Some((dir, is_top)) = stack.pop() {
        if ctx.cancelled() {
            break;
        }
        let listing = match ctx.source.read_dir(&dir) {
            Ok(l) => l,
            Err(e) => {
                ctx.error(&dir, &e);
                continue;
            }
        };
        for (name, e) in &listing.errors {
            ctx.error(&dir.join(name), e);
        }
        if is_top {
            summary.direct = (listing.entries.len() + listing.errors.len()) as i64;
        }
        for entry in &listing.entries {
            match entry.kind {
                EntryKind::File => {
                    summary.size += entry.size;
                    summary.files += 1;
                }
                EntryKind::Dir => stack.push((dir.join(&entry.name), false)),
                EntryKind::Link { .. } => {}
            }
        }
    }
    ctx.progress
        .files
        .fetch_add(summary.files, Ordering::Relaxed);
    ctx.progress
        .bytes
        .fetch_add(summary.size, Ordering::Relaxed);
    summary
}
