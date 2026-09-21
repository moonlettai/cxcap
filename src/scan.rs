//! Per-file parallel scan: read-only walk, kind dispatch, metrics fill.
//! Library code so examples and integration tests exercise the real path.

use crate::dispatch::{self, FileMetrics};
use crate::py::{PyFunc, PyImport};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Directories never scanned (vendored tooling, build output, caches).
/// Must stay in sync with the Python implementation's SKIP_DIRS.
pub const SKIP_DIRS: &[&str] = &[
    ".git", ".hg", ".svn", "__pycache__", ".mypy_cache", ".pytest_cache", ".ruff_cache",
    ".tox", ".venv", "venv", ".env", "node_modules", "dist", "build", ".next", "target",
    ".parcel-cache", ".turbo", "coverage", ".nyc_output", ".astro", ".yarn", "vendor",
    "_vendor",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Other,
    Python,
    Js,
    Rs,
    Test,
    Doc,
    Config,
}

/// True for production code kinds (the change targets hotspots rank and
/// clones hash). Verification surface (`test`), docs, config, and unscored
/// files are never production, wherever the check is needed.
pub fn is_prod_kind(kind: &str) -> bool {
    matches!(kind, "python" | "js" | "rust")
}

/// Production paths for cycle membership checks: verification surface
/// (tests/examples/generated/type-declarations) never qualifies, wherever
/// the check is needed (warning severity, verdict floor share one source).
pub fn prod_path_set(files: &[FileRec]) -> std::collections::HashSet<&str> {
    files
        .iter()
        .filter(|f| is_prod_kind(f.kind))
        .map(|f| f.path.as_str())
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct FileRec {
    pub path: String,
    pub bytes: u64,
    pub loc: u32,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<&'static str>,
    pub funcs: usize,
    pub classes: usize,
    pub complexity: u32,
    pub max_nesting: u32,
    pub fan_out_in: u32,
    pub fan_out_ex: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_func_cx: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_params: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imports: Option<Imports>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_only_imports: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_funcs: Option<Vec<PyFunc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_error: Option<String>,
    // Filled by the relationships increment; absent until then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fan_in: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coupling: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dup_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotspot: Option<f64>,
    /// All symbol names for ephemeral lexical ranking (never serialized).
    #[serde(skip_serializing)]
    pub func_names: Option<Vec<String>>,
}

/// Python recs use one "imports" key holding either import dicts (Python)
/// or path strings (JS) — same key, untagged union.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Imports {
    Py(Vec<PyImport>),
    Js(Vec<String>),
    /// Recorded but unresolved: relate.rs draws no edges yet (a later
    /// increment maps crate/super/self paths onto the module tree).
    Rs(Vec<String>),
}

/// Line count mirroring len(text.splitlines()) on translated text.
pub fn py_line_count(text: &str) -> usize {
    dispatch::splitlines(text).len()
}

/// Last-dot extension of the basename, mirroring Python's os.path.splitext:
/// leading dots of dotfiles never start an extension (`.gitignore` -> "").
pub fn ext_of(rel: &str) -> String {
    let base = rel
        .rfind(['/', '\\'])
        .map(|i| &rel[i + 1..])
        .unwrap_or(rel);
    let stripped = base.trim_start_matches('.');
    if !stripped.contains('.') {
        return String::new();
    }
    match base.rfind('.') {
        Some(i) => base[i..].to_lowercase(),
        None => String::new(),
    }
}

/// Read-only directory walk. Never follows symlinks; counts them instead.
pub fn iter_files(root: &Path, out: &mut Vec<(String, PathBuf, u64)>, symlinks: &mut u64) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let entries = match fs::read_dir(&cur) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                *symlinks += 1;
                continue;
            }
            if ft.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if SKIP_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                stack.push(entry.path());
            } else if ft.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let rel = match entry.path().strip_prefix(root) {
                    Ok(p) => p.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };
                out.push((rel, entry.path(), size));
            }
        }
    }
}

pub struct ScanResult {
    pub rec: FileRec,
    pub kind: Kind,
    /// Code-LOC delta (analyze_text return: 0 for non-code).
    pub delta: u32,
    pub bytes: u64,
    pub ext_counted: String,
    pub oversize: bool,
    pub unreadable: bool,
    /// Clone-detection input for eligible prod-code files.
    pub clone: Option<(Vec<String>, usize)>,
}

fn blank_rec(rel: &str, size: u64) -> FileRec {
    FileRec {
        path: rel.to_string(),
        bytes: size,
        loc: 0,
        kind: "other",
        skipped: None,
        funcs: 0,
        classes: 0,
        complexity: 0,
        max_nesting: 0,
        fan_out_in: 0,
        fan_out_ex: 0,
        max_func_cx: None,
        max_params: None,
        imports: None,
        type_only_imports: None,
        top_funcs: None,
        parse_error: None,
        fan_in: None,
        coupling: None,
        dup_lines: None,
        hotspot: None,
        func_names: None,
    }
}

fn fill_rec(rec: &mut FileRec, m: &FileMetrics) {
    rec.loc = m.loc;
    rec.kind = m.kind;
    rec.funcs = m.funcs;
    rec.classes = m.classes;
    rec.complexity = m.complexity;
    rec.max_nesting = m.max_nesting;
    rec.fan_out_in = m.fan_out_in;
    rec.fan_out_ex = m.fan_out_ex;
    rec.max_func_cx = m.max_func_cx;
    rec.max_params = m.max_params;
    rec.imports = match (&m.imports_py, &m.imports_js, &m.imports_rs) {
        (Some(v), _, _) => Some(Imports::Py(v.clone())),
        (_, Some(v), _) => Some(Imports::Js(v.clone())),
        (_, _, Some(v)) => Some(Imports::Rs(v.clone())),
        _ => None,
    };
    rec.type_only_imports = m.type_only_js.clone();
    rec.top_funcs = m.top_funcs.clone();
    rec.parse_error = m.parse_error.clone();
    if !m.func_names.is_empty() {
        rec.func_names = Some(m.func_names.clone());
    }
}

pub fn scan_one(
    entry: &(String, PathBuf, u64),
    max_bytes: u64,
    tops: &HashSet<String>,
) -> ScanResult {
    let (rel, ap, size) = entry;
    let mut ext = ext_of(rel);
    let ext_counted = if ext.is_empty() {
        "(none)".into()
    } else {
        ext.clone()
    };
    let mut rec = blank_rec(rel, *size);
    if *size > max_bytes {
        rec.skipped = Some("oversize");
        return ScanResult {
            rec,
            kind: Kind::Other,
            delta: 0,
            bytes: *size,
            ext_counted,
            oversize: true,
            unreadable: false,
            clone: None,
        };
    }
    let is_code = dispatch::is_code_ext(&ext);
    let is_doc = dispatch::is_doc_ext(&ext);
    if !(is_code || is_doc || dispatch::is_cfg_ext(&ext)) {
        // Extensionless executables with a Python shebang are Python;
        // content-sniffed, not repo-specific. The sniff reads the file
        // itself; an unreadable file simply stays "other" (no unreadable
        // count — same as the reference).
        if ext.is_empty() {
            match fs::read(ap) {
                Ok(b) => {
                    if dispatch::has_py_shebang_text(&dispatch::decode_text(&b)) {
                        ext = ".py".to_string();
                    } else {
                        return ScanResult {
                            rec,
                            kind: Kind::Other,
                            delta: 0,
                            bytes: *size,
                            ext_counted,
                            oversize: false,
                            unreadable: false,
                            clone: None,
                        };
                    }
                }
                Err(_) => {
                    return ScanResult {
                        rec,
                        kind: Kind::Other,
                        delta: 0,
                        bytes: *size,
                        ext_counted,
                        oversize: false,
                        unreadable: false,
                        clone: None,
                    };
                }
            }
        } else {
            return ScanResult {
                rec,
                kind: Kind::Other,
                delta: 0,
                bytes: *size,
                ext_counted,
                oversize: false,
                unreadable: false,
                clone: None,
            };
        }
    }
    let bytes = match fs::read(ap) {
        Ok(b) => b,
        Err(_) => {
            rec.skipped = Some("unreadable");
            return ScanResult {
                rec,
                kind: Kind::Other,
                delta: 0,
                bytes: *size,
                ext_counted: ext,
                oversize: false,
                unreadable: true,
                clone: None,
            };
        }
    };
    // Same contract as the reference text-mode read: utf-8-sig BOM strip,
    // lossy decode, universal newlines.
    let text = dispatch::decode_text(&bytes);
    let loc = py_line_count(&text) as u32;
    rec.loc = loc;
    if is_doc {
        rec.kind = "doc";
        return ScanResult {
            rec,
            kind: Kind::Doc,
            delta: 0,
            bytes: *size,
            ext_counted: ext,
            oversize: false,
            unreadable: false,
            clone: None,
        };
    }
    let (m, delta) = dispatch::analyze_text(&ext, &text, loc, tops);
    fill_rec(&mut rec, &m);
    let mut kind = match m.kind {
        "python" => Kind::Python,
        "js" => Kind::Js,
        "rust" => Kind::Rs,
        "config" => Kind::Config,
        _ => Kind::Other,
    };
    if m.kind == "config" {
        // delta is 0; caller routes full loc to config counters.
    } else if matches!(kind, Kind::Python | Kind::Js | Kind::Rs)
        && (dispatch::is_test_file(rel) || dispatch::has_generated_header(&text) || {
            // Minified-ness is a property of the SCORED text: notebook JSON
            // boilerplate and SFC markup are unscored wrappers, so judging
            // the raw file would flag bulky outputs/markup instead of code
            // (same rule as clone hashing: hash/score what you analyze).
            match &m.clone_source {
                dispatch::CloneSource::Analyzed(script) => dispatch::is_minified(script),
                dispatch::CloneSource::FileText => dispatch::is_minified(&text),
                dispatch::CloneSource::Skip => false,
            }
        })
    {
        rec.kind = "test"; // same metrics, different change economics
        kind = Kind::Test;
    }
    let clone = if is_prod_kind(rec.kind)
        && rec.loc <= dispatch::CLONE_MAX_FILE_LOC
    {
        // Hash the analyzed text, not the raw file: notebook JSON and SFC
        // markup are unscored wrappers (see CloneSource).
        match &m.clone_source {
            dispatch::CloneSource::Skip => None,
            dispatch::CloneSource::FileText => {
                let (windows, kept) = dispatch::clone_windows(&text);
                Some((windows, kept))
            }
            dispatch::CloneSource::Analyzed(script) => {
                let (windows, kept) = dispatch::clone_windows(script);
                Some((windows, kept))
            }
        }
    } else {
        None
    };
    ScanResult {
        rec,
        kind,
        delta,
        bytes: *size,
        ext_counted: ext,
        oversize: false,
        unreadable: false,
        clone,
    }
}

