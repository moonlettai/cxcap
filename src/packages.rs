//! Package roots: local top-level names and workspace packages.
//!
//! Heuristics for resolving absolute and workspace imports against the
//! scanned tree. Entries-derived and skip-aware; no extra I/O.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

// --- local toplevels + workspace packages ---

fn is_ident(s: &str) -> bool {
    // Mirror of str.isidentifier for the walk-up loop (ASCII fast path
    // covers every real package dir; fall back to alphanumeric check).
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_alphabetic() => (),
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// Scan one directory for top-level names (subdirs plus `.py` files).
/// Returns false when the directory cannot be read (caller skips follow-ups
/// gated on the root scan, mirroring the reference exactly). `skip_dunder`
/// filters `__`-prefixed entries (only the `src/` scan needs it).
fn collect_tops(dir: &Path, skip_dunder: bool, tops: &mut HashSet<String>) -> bool {
    let it = match fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return false,
    };
    for e in it.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || (skip_dunder && name.starts_with("__")) {
            continue;
        }
        let ft = match e.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            tops.insert(name);
        } else if ft.is_file() && name.ends_with(".py") {
            tops.insert(name[..name.len() - 3].to_string());
        }
    }
    true
}

/// Heuristic set of local top-level package names for absolute-import matching.
pub fn local_toplevels(root: &Path) -> HashSet<String> {
    let mut tops = HashSet::new();
    // Root and `<root>/src` scans share one helper: same entries (dirs plus
    // `.py` files), differing only in the `src/` dunder guard (source dirs
    // like `__pycache__` never name packages, while root-level `__init__.py`
    // walk-up below handles the package chain separately).
    if collect_tops(root, false, &mut tops) {
        let src = root.join("src");
        if src.is_dir() {
            collect_tops(&src, true, &mut tops);
        }
    }
    // Subdirectory audits: walk up the package chain (__init__.py present).
    // os.path.abspath normalizes lexically (no symlink resolution) — mirror
    // that exactly so walk-up names agree with the reference.
    let mut p = {
        let abs = if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(root)
        };
        let mut norm = std::path::PathBuf::new();
        for comp in abs.components() {
            use std::path::Component::*;
            match comp {
                CurDir => {}
                ParentDir => {
                    norm.pop();
                }
                _ => norm.push(comp),
            }
        }
        norm
    };
    for _ in 0..7 {
        match p.parent() {
            Some(par) => p = par.to_path_buf(),
            None => break,
        }
        let base = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if base.is_empty() || !is_ident(&base) {
            break;
        }
        tops.insert(base);
        if !p.join("__init__.py").is_file() {
            break;
        }
    }
    tops
}

/// Nested src-layout package names from the scanned entries:
/// `<...>/src/<pkg>/__init__.py` (any depth). The root-level rule above
/// covers single-distribution `<root>/src/<pkg>`; multi-distribution
/// monorepos (`airflow-core/src/airflow`, `task-sdk/src/airflow`,
/// `providers/*/src/airflow`, ...) keep every absolute `pkg.*` import
/// below the tops gate otherwise — coupling, cycles, and focus blast
/// radius go blind on the whole repo. The `__init__.py` witness is the
/// guard: JS `src/` source dirs (components, pages, ...) and PEP 420
/// namespace stubs without an init never join, so no new false heads.
/// Derived from entries (already skip-aware): no extra I/O, skipped
/// dirs (node_modules, .venv, ...) cannot contribute.
pub fn nested_src_toplevels(entries: &[(String, std::path::PathBuf, u64)]) -> HashSet<String> {
    let mut tops = HashSet::new();
    for (rel, _, _) in entries {
        let segs: Vec<&str> = rel.split('/').collect();
        if segs.len() >= 4
            && segs[segs.len() - 1] == "__init__.py"
            && segs[segs.len() - 3] == "src"
        {
            let pkg = segs[segs.len() - 2];
            if is_ident(pkg) && !pkg.starts_with("__") {
                tops.insert(pkg.to_string());
            }
        }
    }
    tops
}
/// Map JS workspace package names (package.json "name" fields) to their
/// directory relative to root. `entries` is the sorted (rel, path, size) walk.
pub fn find_workspace_packages(
    entries: &[(String, std::path::PathBuf, u64)],
) -> std::collections::HashMap<String, String> {
    let mut found = std::collections::HashMap::new();
    for (rel, ap, _size) in entries {
        let base = rel.rsplit('/').next().unwrap_or(rel);
        if base != "package.json" || rel.matches('/').count() > 4 {
            continue;
        }
        // Strict utf-8-sig read (single contract in resolve.rs):
        // undecodable manifests are skipped, never lossy-decoded.
        let d = match crate::resolve::read_json_strict(ap) {
            Some(d) => d,
            None => continue,
        };
        let name = d.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if !name.is_empty() && name.split('/').count() <= 2 {
            let dir = match rel.rfind('/') {
                Some(i) => rel[..i].to_string(),
                None => ".".to_string(),
            };
            // first name wins
            found.entry(name.to_string()).or_insert(dir);
        }
    }
    found
}
