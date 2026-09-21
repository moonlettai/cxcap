//! Shared test paths without absolute bakes.
//!
//! Everything derives from the running test executable
//! (`<pkg>/target/<profile>/deps/<test>-<hash>`), so the checkout can be
//! renamed or relocated without stale baked-in paths, and no
//! machine-specific absolute path is embedded in test logic. No mocks,
//! no stubs — the real binary, located relatively.

use std::path::PathBuf;

/// Directory of the running test executable (`target/<profile>/deps`).
fn exe_dir() -> PathBuf {
    let mut p = std::env::current_exe().expect("test executable path");
    p.pop();
    p
}

/// Directory holding the built `cxcap` binary (`target/<profile>`).
pub fn target_dir() -> PathBuf {
    let mut p = exe_dir();
    if p.file_name().map(|n| n == "deps").unwrap_or(false) {
        p.pop();
    }
    p
}

/// The `cxcap` binary under test.
pub fn bin() -> PathBuf {
    target_dir().join("cxcap")
}

/// Package root (the checkout directory), whatever it is named or
/// wherever it sits on disk.
pub fn pkg_root() -> PathBuf {
    let mut p = exe_dir();
    // deps -> profile -> target -> package root.
    if p.file_name().map(|n| n == "deps").unwrap_or(false) {
        p.pop();
    }
    p.pop();
    p.pop();
    p
}

// --- evaluation-harness helpers (verification surface, not production) ---
//
// Moved out of `cxcap::lexical` so production code carries only the shipped
// BM25 ranker. The literal baseline + Recall@K exist to test whether BM25
// beats the cheaper alternative; their only consumers are integration tests.

/// Simple literal baseline: count of query tokens appearing as substrings.
pub fn rank_literal(intent: &str, files: &[cxcap::scan::FileRec], top_k: usize) -> Vec<(String, f64)> {
    let q = cxcap::lexical::tokenize(intent);
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(String, f64)> = Vec::new();
    for rec in files.iter().filter(|r| cxcap::scan::is_prod_kind(r.kind)) {
        // Same names source as production ranking: func_names is populated
        // exactly when the file defines functions.
        let names = rec
            .func_names
            .as_ref()
            .map(|ns| ns.join(" "))
            .unwrap_or_default();
        let hay = format!("{} {}", rec.path.to_lowercase(), names.to_lowercase());
        let mut s = 0.0;
        for term in q.iter() {
            if hay.contains(term.as_str()) {
                s += 1.0;
            }
        }
        if s > 0.0 {
            scored.push((rec.path.clone(), s));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap()
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(top_k);
    scored
}

/// Recall@K: `actual` = files eventually changed; `ranked` = predicted paths.
pub fn recall_at_k(ranked: &[String], actual: &[String], k: usize) -> f64 {
    if actual.is_empty() {
        return 1.0;
    }
    let top: std::collections::HashSet<&str> =
        ranked.iter().take(k).map(|s| s.as_str()).collect();
    let hit = actual.iter().filter(|a| top.contains(a.as_str())).count();
    hit as f64 / actual.len() as f64
}
