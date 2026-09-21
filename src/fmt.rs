//! Display primitives: Python-compatible number formatting, pluralized
//! counts, and folder-distribution rows. Leaf module: no imports from
//! sibling analysis modules, so dependency direction stays one-way.

use serde::Serialize;

// ---------------------------------------------------------------------------
// Python-compatible float helpers
// ---------------------------------------------------------------------------

/// round(x, 1) with banker's rounding, like Python. Inputs here are small
/// non-negative scores, so the i64 cast is safe.
pub fn py_round1(x: f64) -> f64 {
    let y = x * 10.0;
    let f = y.floor();
    let d = y - f;
    if d < 0.5 {
        f / 10.0
    } else if d > 0.5 {
        (f + 1.0) / 10.0
    } else if (f as i64) % 2 == 0 {
        f / 10.0
    } else {
        (f + 1.0) / 10.0
    }
}

/// Format like Python's f"{x:.1f}": half-even rounding, one decimal.
pub fn py_fmt1(x: f64) -> String {
    format!("{:.1}", py_round1(x))
}

/// Count + noun with correct plurals for user-facing text.
pub(crate) fn n_txt(n: usize, one: &str, many: Option<&str>) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {}", many.unwrap_or(&format!("{one}s")))
    }
}

pub(crate) fn n1(n: usize, one: &str) -> String {
    n_txt(n, one, None)
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderRow {
    pub dir: String,
    pub complexity: u32,
    pub loc: u32,
    pub share: f64,
}

pub(crate) fn folder_of(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 2 {
        parts[..parts.len() - 1].join("/")
    } else {
        "(root)".to_string()
    }
}
