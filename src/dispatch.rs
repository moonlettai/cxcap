//! Per-file kind dispatch, mirroring the reference implementation's
//! `analyze_text` + audit-loop file handling exactly: notebook/SFC
//! extraction, shebang promotion, test/generated split, clone windows,
//! local-toplevel and workspace-package discovery.
//!
//! Anything here that disagrees with Python on real corpus files is a bug
//! in this module — the parity harness decides, not code review.

use crate::py::{PyFunc, PyImport, analyze_python};
use crate::rs::analyze_rust;
use std::collections::HashSet;

// --- extension sets (must stay in sync with the reference) ---

pub fn is_py_ext(ext: &str) -> bool {
    matches!(ext, ".py" | ".pyi")
}
pub fn is_nb_ext(ext: &str) -> bool {
    ext == ".ipynb"
}
pub fn is_js_ext(ext: &str) -> bool {
    matches!(
        ext,
        ".js" | ".jsx" | ".ts" | ".tsx" | ".mjs" | ".cjs" | ".mts" | ".cts"
    )
}
pub fn is_sfc_ext(ext: &str) -> bool {
    matches!(ext, ".astro" | ".vue" | ".svelte")
}
pub fn is_rs_ext(ext: &str) -> bool {
    ext == ".rs"
}
pub fn is_doc_ext(ext: &str) -> bool {
    matches!(ext, ".md" | ".rst" | ".txt" | ".adoc")
}
pub fn is_cfg_ext(ext: &str) -> bool {
    matches!(ext, ".json" | ".yaml" | ".yml" | ".toml" | ".ini" | ".cfg")
}
pub fn is_code_ext(ext: &str) -> bool {
    is_py_ext(ext) || is_nb_ext(ext) || is_js_ext(ext) || is_sfc_ext(ext) || is_rs_ext(ext)
}

// Submodule facades: implementation lives in coherent units; these paths
// stay stable so callers never churn on refactors.
pub use crate::extract::{notebook_source, sfc_extract};
pub use crate::packages::{find_workspace_packages, local_toplevels, nested_src_toplevels};
pub use crate::surfaces::{
    CLONE_MAX_FILE_LOC, CLONE_MIN_LINES, clone_windows, has_generated_header, is_minified,
    is_test_file,
};
pub use crate::text::{decode_text, has_py_shebang_text, splitlines};


// --- metrics fill ---

/// Fully-filled per-file metrics; mirrors the reference `rec` dict keys for
/// code files (optional keys serialize only when present).
#[derive(Debug, Default)]
pub struct FileMetrics {
    pub kind: &'static str,
    pub loc: u32,
    pub funcs: usize,
    pub classes: usize,
    pub complexity: u32,
    pub max_nesting: u32,
    pub fan_out_in: u32,
    pub fan_out_ex: u32,
    pub max_func_cx: Option<u32>,
    pub max_params: Option<u32>,
    pub imports_py: Option<Vec<PyImport>>,
    pub imports_js: Option<Vec<String>>,
    pub imports_rs: Option<Vec<String>>,
    pub type_only_js: Option<Vec<String>>,
    pub top_funcs: Option<Vec<PyFunc>>,
    pub parse_error: Option<String>,
    pub clone_source: CloneSource,
    /// All function/symbol names before top-5 truncation, for ephemeral
    /// lexical ranking only (never serialized; reuses already-computed AST data).
    pub func_names: Vec<String>,
}

/// Which text copy-paste detection hashes for a file. Hashing must cover
/// what the audit scores: notebook JSON boilerplate and SFC markup are
/// unscored wrapper text, so hashing them manufactures shared windows
/// (and absurd dup_lines) out of nothing. Markup-only SFC files contribute
/// no code at all and are skipped.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum CloneSource {
    #[default]
    FileText,
    Analyzed(String),
    Skip,
}

/// Kind dispatch for one file's text. Returns (metrics, code-LOC delta).
/// `full_loc` is len(text.splitlines()) computed by the caller.
pub fn analyze_text(
    ext: &str,
    text: &str,
    full_loc: u32,
    tops: &HashSet<String>,
) -> (FileMetrics, u32) {
    let mut m = FileMetrics {
        loc: full_loc,
        ..Default::default()
    };
    if is_doc_ext(ext) {
        m.kind = "doc";
        return (m, 0);
    }
    if is_cfg_ext(ext) {
        m.kind = "config";
        return (m, 0);
    }
    if is_nb_ext(ext) {
        m.kind = "python";
        match notebook_source(text) {
            None => {
                m.parse_error = Some("invalid notebook JSON".to_string());
                m.loc = 0;
                m.clone_source = CloneSource::Skip;
                return (m, 0);
            }
            Some(script) => {
                m.loc = splitlines(&script).len() as u32;
                let (mut r, d) = analyze_python_text(&script, m, tops);
                r.clone_source = CloneSource::Analyzed(script);
                return (r, d);
            }
        }
    }
    if is_py_ext(ext) {
        m.kind = "python";
        return analyze_python_text(text, m, tops);
    }
    if is_rs_ext(ext) {
        m.kind = "rust";
        return analyze_rust_text(text, m);
    }
    // JS/TS and single-file components share the AST scorer (grammars
    // chosen by extension; SFC script scores as TypeScript).
    m.kind = "js";
    if is_sfc_ext(ext) {
        let (script, found) = sfc_extract(text);
        if !found {
            let delta = m.loc;
            m.clone_source = CloneSource::Skip;
            return (m, delta); // markup-only: LOC counts, scores stay 0
        }
        fill_ts(&mut m, &script, crate::ts::TsLang::TypeScript);
        m.clone_source = CloneSource::Analyzed(script);
    } else if is_js_ext(ext) {
        let lang = match ext {
            ".tsx" => crate::ts::TsLang::Tsx,
            ".ts" | ".mts" | ".cts" => crate::ts::TsLang::TypeScript,
            _ => crate::ts::TsLang::JavaScript,
        };
        fill_ts(&mut m, text, lang);
    } else {
        // Unreachable: every is_code_ext member is wired above. Kept for
        // totality; scores nothing.
        m.kind = "other";
    }
    let delta = m.loc;
    (m, delta)
}

fn analyze_python_text(
    text: &str,
    mut m: FileMetrics,
    tops: &HashSet<String>,
) -> (FileMetrics, u32) {
    m.kind = "python";
    match analyze_python(text) {
        Err(e) => {
            m.parse_error = Some(e);
            let delta = m.loc;
            (m, delta)
        }
        Ok(r) => {
            m.funcs = r.funcs;
            m.classes = r.classes;
            m.complexity = r.complexity;
            m.max_nesting = r.max_nesting;
            m.max_func_cx = Some(r.max_func_cx);
            m.max_params = Some(r.max_params);
            // Runtime coupling only: annotation-only references
            // (`if TYPE_CHECKING:`) never break at runtime, so they count
            // toward neither split — same rule as fan_in (#78). The stored
            // import list is untouched (cycles/focus still see type-only).
            let runtime: Vec<_> = r.imports.iter().filter(|i| !i.type_only).collect();
            let extra_in = runtime
                .iter()
                .filter(|i| i.level == 0 && tops.contains(i.module.split('.').next().unwrap_or("")))
                .count() as u32;
            let internal = runtime.iter().filter(|i| i.level > 0).count() as u32;
            m.fan_out_in = internal + extra_in;
            m.fan_out_ex = runtime.len() as u32 - internal - extra_in;
            m.imports_py = Some(r.imports);
            // func_details: stable sort by -complexity, top 5.
            let mut funcs = r.func_details;
            m.func_names = r.func_names;
            funcs.sort_by(|a, b| b.complexity.cmp(&a.complexity));
            funcs.truncate(5);
            m.top_funcs = Some(funcs);
            let delta = m.loc;
            (m, delta)
        }
    }
}

fn analyze_rust_text(text: &str, mut m: FileMetrics) -> (FileMetrics, u32) {
    m.kind = "rust";
    match analyze_rust(text) {
        Err(e) => {
            m.parse_error = Some(e);
            let delta = m.loc;
            (m, delta)
        }
        Ok(r) => {
            m.funcs = r.funcs;
            m.classes = r.classes;
            m.complexity = r.complexity;
            m.max_nesting = r.max_nesting;
            m.max_func_cx = Some(r.max_func_cx);
            m.max_params = Some(r.max_params);
            // Crate-relative paths are internal (2018+ paths: bare heads
            // are extern crates). Edge RESOLUTION is a later increment —
            // relate.rs draws no Rust edges yet — but the fan split already
            // distinguishes hub shape honestly.
            let internal = r
                .imports
                .iter()
                .filter(|p| {
                    p.starts_with("crate::") || p.starts_with("super::") || p.starts_with("self::")
                })
                .count() as u32;
            m.fan_out_in = internal;
            m.fan_out_ex = r.imports.len() as u32 - internal;
            m.imports_rs = Some(r.imports);
            let mut funcs: Vec<PyFunc> = r
                .func_details
                .into_iter()
                .map(|f| PyFunc {
                    name: f.name,
                    lineno: f.lineno,
                    end: f.end,
                    params: f.params,
                    complexity: f.complexity,
                    length: f.length,
                })
                .collect();
            m.func_names = r.func_names;
            funcs.sort_by(|a, b| b.complexity.cmp(&a.complexity));
            funcs.truncate(5);
            m.top_funcs = Some(funcs);
            let delta = m.loc;
            (m, delta)
        }
    }
}




/// Clean a harvested JS/TS module specifier: strip `#fragment`, drop
/// bundler asset queries (`?raw`/`?url`/`?inline` are file contents, not
/// module edges — the import_hygiene rule), keep the path otherwise.
fn clean_js_import(raw: &str) -> Option<String> {
    let path = raw.split('#').next().unwrap_or("");
    if let Some((p, q)) = path.split_once('?') {
        let first = q.split('&').next().unwrap_or("").split('=').next().unwrap_or("");
        if matches!(first, "raw" | "url" | "inline") {
            return None;
        }
        if p.is_empty() {
            return None;
        }
        return Some(p.to_string());
    }
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Fill from the tree-sitter JS/TS analyzer. Imports flow into the same
/// `imports_js`/`type_only_js` fields, so the relationship machinery is
/// untouched.
fn fill_ts(m: &mut FileMetrics, script: &str, lang: crate::ts::TsLang) {
    let r = crate::ts::analyze_ts(script, lang);
    m.funcs = r.funcs;
    m.classes = r.classes;
    m.complexity = r.complexity;
    m.max_nesting = r.max_nesting;
    m.max_func_cx = Some(r.max_func_cx);
    m.max_params = Some(r.max_params);
    let imports: Vec<String> = r.imports.into_iter().filter_map(|p| clean_js_import(&p)).collect();
    let internal = imports.iter().filter(|p| p.starts_with('.')).count() as u32;
    m.fan_out_in = internal;
    m.fan_out_ex = imports.len() as u32 - internal;
    m.imports_js = Some(imports);
    let mut to: Vec<String> = r.type_only_imports.into_iter().filter_map(|p| clean_js_import(&p)).collect();
    to.sort();
    to.dedup();
    m.type_only_js = Some(to);
    let mut funcs: Vec<PyFunc> = r
        .func_details
        .into_iter()
        .map(|f| PyFunc {
            name: f.name,
            lineno: f.lineno,
            end: f.end,
            params: f.params,
            complexity: f.complexity,
            length: f.length,
        })
        .collect();
    funcs.sort_by(|a, b| b.complexity.cmp(&a.complexity));
    m.func_names = r.func_names;
    funcs.truncate(5);
    m.top_funcs = Some(funcs);
}
