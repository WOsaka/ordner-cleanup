//! Abschnitt „Ordner-Duplikate“ des Berichts: exakte Gruppen (oberste Ebene) und teilweise
//! gleiche Paare. Nutzt dieselbe Erkennung wie `plan dedupe-dirs`, aber ohne Schutzregeln.

use anyhow::Result;

use super::{FolderDupGroup, FolderDuplicatesReport, PartialPair};
use crate::analysis::folder_dups::{self, FolderDupOptions, PartialOptions};
use crate::change::tree::Tree;
use crate::index::Index;

pub(super) fn build(
    index: &Index,
    root_key: &str,
    partial_threshold: f64,
) -> Result<FolderDuplicatesReport> {
    let tree = Tree::new(index.dirs_under(root_key)?, index.files_under(root_key)?);
    let hashes = index.file_hashes_under(root_key)?;
    let analysis = folder_dups::analyze(
        &tree,
        &hashes,
        &FolderDupOptions {
            root_key: root_key.to_string(),
        },
    );
    let path_of = |key: &str| {
        tree.row(key)
            .map_or_else(|| key.to_string(), |r| r.path.clone())
    };

    let mut groups: Vec<FolderDupGroup> = folder_dups::top_level_groups(&analysis.groups)
        .into_iter()
        .map(|g| {
            let mut dirs: Vec<String> = g.members.iter().map(|m| path_of(m)).collect();
            dirs.sort();
            FolderDupGroup {
                reclaimable: g.bytes * (dirs.len() as u64 - 1),
                dirs,
                files: g.files,
                bytes: g.bytes,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.reclaimable
            .cmp(&a.reclaimable)
            .then_with(|| a.dirs.cmp(&b.dirs))
    });

    let partial = folder_dups::partial_pairs(
        &tree,
        &hashes,
        &analysis.groups,
        &PartialOptions::new(root_key.to_string(), partial_threshold),
    );
    Ok(FolderDuplicatesReport {
        total_reclaimable: groups.iter().map(|g| g.reclaimable).sum(),
        groups,
        partial: partial
            .pairs
            .into_iter()
            .map(|p| PartialPair {
                a: path_of(&p.a),
                b: path_of(&p.b),
                shared_bytes: p.shared_bytes,
                ratio: p.ratio,
            })
            .collect(),
        partial_threshold,
        partial_incomplete: partial.incomplete,
        unverifiable: analysis.unverifiable,
    })
}
