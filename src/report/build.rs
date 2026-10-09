use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::Result;

use super::rows::{load_dirs, load_errors, load_files, DRow, FRow};
use super::*;
use crate::analysis::age::{self, AgeClass};
use crate::analysis::filetypes::{self, Category};
use crate::analysis::problems::{self, ProblemCtx};
use crate::analysis::similar;
use crate::analysis::structure::{self, DirFacts, Thresholds};
use crate::scan::source::TICKS_PER_SEC;

pub struct ReportParams {
    pub generated_at: String,
    /// Aktuelle Zeit in 100-ns-Ticks seit der Unix-Epoche
    pub now_ticks: i64,
    pub top: usize,
    pub old_after_days: i64,
    pub thresholds: Thresholds,
    pub problem_ctx: ProblemCtx,
    /// Schwelle für „teilweise gleiche“ Ordner (`dedupe_dirs_partial_threshold`)
    pub partial_threshold: f64,
}

fn name_of(path: &str) -> String {
    path.rsplit('\\')
        .find(|p| !p.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn file_item(f: &FRow, now_ticks: i64) -> FileItem {
    FileItem {
        path: f.path.clone(),
        size: f.size,
        mtime: f.mtime / TICKS_PER_SEC,
        age_days: age::age_days(f.mtime, now_ticks),
        hidden: f.attrs.is_hidden(),
        system: f.attrs.is_system(),
        cloud_only: f.cloud,
    }
}

fn by_size_then_path<T>(items: &mut [T], size: impl Fn(&T) -> u64, path: impl Fn(&T) -> &str) {
    items.sort_by(|a, b| size(b).cmp(&size(a)).then_with(|| path(a).cmp(path(b))));
}

struct TreeCtx<'a> {
    dirs: &'a [DRow],
    children: HashMap<&'a str, Vec<usize>>,
    cum: HashMap<&'a str, (u64, u64)>,
}

impl TreeCtx<'_> {
    fn node(&self, i: usize) -> TreeNode {
        let d = &self.dirs[i];
        let (size, files) = self.cum.get(d.key.as_str()).copied().unwrap_or((0, 0));
        let mut children: Vec<TreeNode> = self
            .children
            .get(d.key.as_str())
            .map(|c| c.iter().map(|&j| self.node(j)).collect())
            .unwrap_or_default();
        by_size_then_path(&mut children, |n| n.size, |n| &n.path);
        TreeNode {
            name: name_of(&d.path),
            path: d.path.clone(),
            size,
            files,
            summary: d.summary,
            link: d.is_link,
            children,
        }
    }
}

pub fn build(index: &Index, root: &ReportRoot, p: &ReportParams) -> Result<Report> {
    let files = load_files(index, &root.dir_key)?;
    let dirs = load_dirs(index, &root.dir_key)?;
    let errors = load_errors(index, &root.dir_key)?;
    let real_files: Vec<&FRow> = files.iter().filter(|f| !f.is_link).collect();

    // --- Größenbaum und Aggregate ---
    let mut own: HashMap<&str, (u64, u64)> = HashMap::new();
    for f in &real_files {
        let e = own.entry(f.dir_key.as_str()).or_default();
        e.0 += if f.counted { f.size } else { 0 };
        e.1 += 1;
    }
    for d in dirs.iter().filter(|d| d.summary) {
        let e = own.entry(d.key.as_str()).or_default();
        e.0 += d.summary_size;
        e.1 += d.summary_files;
    }
    let mut order: Vec<usize> = (0..dirs.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(dirs[i].depth));
    let dir_keys: HashSet<&str> = dirs.iter().map(|d| d.key.as_str()).collect();
    let mut cum = own.clone();
    for &i in &order {
        let d = &dirs[i];
        if let (Some(parent), Some(&(s, f))) = (&d.parent, cum.get(d.key.as_str())) {
            if dir_keys.contains(parent.as_str()) {
                let e = cum.entry(parent.as_str()).or_default();
                e.0 += s;
                e.1 += f;
            }
        }
    }
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, d) in dirs.iter().enumerate() {
        if let Some(parent) = &d.parent {
            children.entry(parent.as_str()).or_default().push(i);
        }
    }
    let tree = TreeCtx {
        dirs: &dirs,
        children,
        cum,
    };
    let root_idx = dirs.iter().position(|d| d.key == root.dir_key);
    let root_depth = root_idx.map_or_else(
        || dirs.iter().map(|d| d.depth).min().unwrap_or(0),
        |i| dirs[i].depth,
    );
    let size_tree = match root_idx {
        Some(i) => tree.node(i),
        None => {
            let mut children: Vec<TreeNode> = tree
                .children
                .get(root.dir_key.as_str())
                .map(|c| c.iter().map(|&j| tree.node(j)).collect())
                .unwrap_or_default();
            by_size_then_path(&mut children, |n| n.size, |n| &n.path);
            TreeNode {
                name: root.path.clone(),
                path: root.path.clone(),
                size: children.iter().map(|c| c.size).sum(),
                files: children.iter().map(|c| c.files).sum(),
                summary: false,
                link: false,
                children,
            }
        }
    };

    // --- Übersicht ---
    let summary_size: u64 = dirs
        .iter()
        .filter(|d| d.summary)
        .map(|d| d.summary_size)
        .sum();
    let summary_files: u64 = dirs
        .iter()
        .filter(|d| d.summary)
        .map(|d| d.summary_files)
        .sum();
    let counted_size: u64 = real_files
        .iter()
        .filter(|f| f.counted)
        .map(|f| f.size)
        .sum();
    let total_size = counted_size + summary_size;
    let overview = Overview {
        files: real_files.len() as u64 + summary_files,
        dirs: dirs.len() as u64,
        total_size,
        summary_size,
        summary_files,
        summary_share: if total_size == 0 {
            0.0
        } else {
            summary_size as f64 / total_size as f64
        },
        cloud_only_files: real_files.iter().filter(|f| f.cloud).count() as u64,
        cloud_only_size: real_files
            .iter()
            .filter(|f| f.cloud && f.counted)
            .map(|f| f.size)
            .sum(),
        hidden_files: real_files.iter().filter(|f| f.attrs.is_hidden()).count() as u64,
        system_files: real_files.iter().filter(|f| f.attrs.is_system()).count() as u64,
        links: files.iter().filter(|f| f.is_link).count() as u64
            + dirs.iter().filter(|d| d.is_link).count() as u64,
        error_count: errors.len() as u64,
    };

    // --- Top-Listen ---
    let mut top_files: Vec<FileItem> = real_files
        .iter()
        .map(|f| file_item(f, p.now_ticks))
        .collect();
    by_size_then_path(&mut top_files, |f| f.size, |f| &f.path);
    top_files.truncate(p.top);
    let mut top_dirs: Vec<DirItem> = dirs
        .iter()
        .filter(|d| d.key != root.dir_key && !d.is_link)
        .map(|d| {
            let (size, files) = tree.cum.get(d.key.as_str()).copied().unwrap_or((0, 0));
            DirItem {
                path: d.path.clone(),
                size,
                files,
                summary: d.summary,
            }
        })
        .collect();
    by_size_then_path(&mut top_dirs, |d| d.size, |d| &d.path);
    top_dirs.truncate(p.top);

    // --- Dateitypen ---
    let mut by_ext: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut by_cat: BTreeMap<Category, (u64, u64)> = BTreeMap::new();
    for f in real_files.iter().filter(|f| f.counted) {
        let key = f.ext.clone().unwrap_or_default();
        let e = by_ext.entry(key.clone()).or_default();
        e.0 += 1;
        e.1 += f.size;
        let c = by_cat
            .entry(filetypes::categorize(f.ext.as_deref()))
            .or_default();
        c.0 += 1;
        c.1 += f.size;
    }
    let mut by_extension: Vec<TypeStat> = by_ext
        .into_iter()
        .map(|(k, (count, size))| TypeStat {
            key: if k.is_empty() { "(keine)".into() } else { k },
            count,
            size,
        })
        .collect();
    by_size_then_path(&mut by_extension, |t| t.size, |t| &t.key);
    let by_category = Category::ALL
        .iter()
        .map(|c| {
            let (count, size) = by_cat.get(c).copied().unwrap_or((0, 0));
            TypeStat {
                key: c.label().to_string(),
                count,
                size,
            }
        })
        .collect();

    // --- Alter ---
    let mut class_stats: BTreeMap<AgeClass, (u64, u64)> = BTreeMap::new();
    for f in real_files.iter().filter(|f| f.counted) {
        let e = class_stats
            .entry(age::classify(f.mtime, p.now_ticks))
            .or_default();
        e.0 += 1;
        e.1 += f.size;
    }
    let classes = AgeClass::ALL
        .iter()
        .map(|c| {
            let (count, size) = class_stats.get(c).copied().unwrap_or((0, 0));
            AgeStat {
                label: c.label().to_string(),
                count,
                size,
            }
        })
        .collect();
    let mut old_files: Vec<FileItem> = real_files
        .iter()
        .filter(|f| age::is_old(f.mtime, p.now_ticks, p.old_after_days))
        .map(|f| file_item(f, p.now_ticks))
        .collect();
    by_size_then_path(&mut old_files, |f| f.size, |f| &f.path);
    let old_total_size = real_files
        .iter()
        .filter(|f| f.counted && age::is_old(f.mtime, p.now_ticks, p.old_after_days))
        .map(|f| f.size)
        .sum();

    // --- Exakte Duplikate ---
    let groups: Vec<DupGroupItem> = index
        .exact_duplicate_groups(&root.dir_key)?
        .into_iter()
        .map(|g| DupGroupItem {
            size: g.size as u64,
            instances: g.instances,
            wasted: g.wasted as u64,
            files: g
                .files
                .into_iter()
                .map(|f| DupFileItem {
                    path: f.path,
                    mtime: f.mtime / TICKS_PER_SEC,
                    nlinks: f.nlinks,
                })
                .collect(),
        })
        .collect();
    let duplicates = DuplicatesReport {
        group_count: groups.len(),
        total_wasted: groups.iter().map(|g| g.wasted).sum(),
        groups,
    };

    // --- Doppelte Ordner ---
    let folder_duplicates = super::folder::build(index, &root.dir_key, p.partial_threshold)?;

    // --- Wahrscheinliche Duplikate (Cloud-Beteiligung, nicht verifiziert) ---
    let mut by_name: BTreeMap<(String, u64), Vec<&FRow>> = BTreeMap::new();
    for f in real_files.iter().filter(|f| f.size > 0) {
        by_name
            .entry((f.name.to_lowercase(), f.size))
            .or_default()
            .push(f);
    }
    let mut probable_duplicates: Vec<ProbableGroup> = by_name
        .into_iter()
        .filter(|(_, g)| g.len() >= 2 && g.iter().any(|f| f.cloud))
        .map(|((_, size), g)| ProbableGroup {
            name: g[0].name.clone(),
            size,
            verified: false,
            files: g
                .iter()
                .map(|f| ProbableFile {
                    path: f.path.clone(),
                    cloud_only: f.cloud,
                })
                .collect(),
        })
        .collect();
    by_size_then_path(&mut probable_duplicates, |g| g.size, |g| &g.files[0].path);

    // --- Ähnliche Dateien ---
    let mut per_dir: BTreeMap<&str, Vec<&FRow>> = BTreeMap::new();
    for f in &real_files {
        per_dir.entry(f.dir_key.as_str()).or_default().push(f);
    }
    let dir_paths: HashMap<&str, &str> = dirs
        .iter()
        .map(|d| (d.key.as_str(), d.path.as_str()))
        .collect();
    let mut similar_groups = Vec::new();
    for (dir_key, members) in &per_dir {
        let names: Vec<&str> = members.iter().map(|f| f.name.as_str()).collect();
        for group in similar::group_similar(&names) {
            let hashes: Vec<Option<&Vec<u8>>> = group
                .iter()
                .map(|&i| members[i].full_hash.as_ref())
                .collect();
            let files = group
                .iter()
                .enumerate()
                .map(|(gi, &i)| {
                    let f = members[i];
                    let exact = hashes[gi].is_some_and(|h| {
                        hashes.iter().enumerate().any(|(oj, o)| {
                            oj != gi && *o == Some(h) && members[group[oj]].size == f.size
                        })
                    });
                    SimilarFile {
                        name: f.name.clone(),
                        size: f.size,
                        mtime: f.mtime / TICKS_PER_SEC,
                        exact_duplicate: exact,
                    }
                })
                .collect();
            similar_groups.push(SimilarGroup {
                dir: dir_paths
                    .get(dir_key)
                    .map_or_else(|| dir_key.to_string(), |s| s.to_string()),
                files,
            });
        }
    }

    // --- Struktur ---
    let error_keys: HashSet<String> = errors
        .iter()
        .map(|e| paths::dir_key(Path::new(&e.path)))
        .collect();
    let mut file_counts: HashMap<&str, usize> = HashMap::new();
    for f in &real_files {
        *file_counts.entry(f.dir_key.as_str()).or_default() += 1;
    }
    let mut structure_items = Vec::new();
    for d in &dirs {
        let facts = DirFacts {
            rel_depth: (d.depth - root_depth).max(0) as usize,
            direct_entries: d.direct_entries as usize,
            direct_files: file_counts.get(d.key.as_str()).copied().unwrap_or(0),
            // Links, „Nur Summe“ und Ordner, die nicht gelesen werden konnten, werden nicht bewertet.
            skipped: d.is_link || d.summary || error_keys.contains(&d.key),
        };
        for issue in structure::check_dir(&facts, &p.thresholds) {
            structure_items.push(StructureItem {
                path: d.path.clone(),
                issue: issue.key().to_string(),
                label: issue.label().to_string(),
                direct_entries: d.direct_entries,
                depth: facts.rel_depth as u64,
            });
        }
    }
    structure_items.sort_by(|a, b| a.issue.cmp(&b.issue).then_with(|| a.path.cmp(&b.path)));

    // --- Problemdateien ---
    let mut problem_items = Vec::new();
    for f in &real_files {
        let found = problems::check_file(&f.name, &f.path, f.size, &p.problem_ctx);
        if !found.is_empty() {
            problem_items.push(ProblemItem {
                path: f.path.clone(),
                kind: "file".into(),
                size: f.size,
                problems: found.iter().map(|x| x.key().to_string()).collect(),
                labels: found.iter().map(|x| x.label().to_string()).collect(),
            });
        }
    }
    for d in dirs.iter().filter(|d| d.key != root.dir_key) {
        let found = problems::check_dir(&name_of(&d.path), &d.path);
        if !found.is_empty() {
            problem_items.push(ProblemItem {
                path: d.path.clone(),
                kind: "dir".into(),
                size: 0,
                problems: found.iter().map(|x| x.key().to_string()).collect(),
                labels: found.iter().map(|x| x.label().to_string()).collect(),
            });
        }
    }
    problem_items.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Report {
        meta: Meta {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            generated_at: p.generated_at.clone(),
            root: root.path.clone(),
            scanned_at: root.scanned_at.clone(),
            scan_status: match root.status {
                RootStatus::Complete => "complete",
                RootStatus::Aborted => "aborted",
                RootStatus::Running => "running",
            }
            .to_string(),
            top: p.top,
            old_after_days: p.old_after_days,
            max_depth_warning: p.thresholds.max_depth,
            huge_dir_entries: p.thresholds.huge_entries,
            notice: CONFIDENTIALITY_NOTICE.to_string(),
        },
        overview,
        size_tree,
        top_files,
        top_dirs,
        file_types: FileTypes {
            by_extension,
            by_category,
        },
        age: AgeReport {
            classes,
            old_after_days: p.old_after_days,
            old_total_size,
            old_files,
        },
        duplicates,
        folder_duplicates,
        probable_duplicates,
        similar: similar_groups,
        structure: structure_items,
        problems: problem_items,
        errors,
        history: None,
        template: None,
        content: None,
    })
}
