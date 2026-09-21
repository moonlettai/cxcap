//! Surface classification: generated, minified, test, and copy-paste input.
//!
//! Generic evidence only (directories, headers, layout, names): decides
//! whether a file counts as production, verification, or generated surface,
//! and builds normalized windows for clone detection.

use crate::text::splitlines;

// --- generated-header sniff ---

const COMMENT_PREFIXES: &[&str] = &["#", "//", "*", "/*", "<!--", "\"\"\"", "'''", "--"];

fn generated_re() -> regex::Regex {
    regex::Regex::new(
        r"(?i)auto-?generated|automatically generated|this file (is|was) generated|(it is|code) generated (from|by)|generated file|generated with",
    )
    .unwrap()
}

/// Rebuilt-output declarations (`do not edit…rebuild`, possibly on
/// different header lines): matched across the whole comment header, not
/// per line, and only when BOTH halves are present.
fn rebuilt_re() -> (regex::Regex, regex::Regex) {
    (
        regex::Regex::new(r"(?i)do not (edit|modify)|never edit").unwrap(),
        regex::Regex::new(r"(?i)rebui[lt]|regenerat").unwrap(),
    )
}

pub fn has_generated_header(text: &str) -> bool {
    let re = generated_re();
    let mut header: Vec<&str> = Vec::new();
    for line in splitlines(text).iter().take(12) {
        let s = line.trim();
        if !s.is_empty()
            && COMMENT_PREFIXES.iter().any(|p| s.starts_with(p))
            && re.is_match(s)
        {
            return true;
        }
        if !s.is_empty() && COMMENT_PREFIXES.iter().any(|p| s.starts_with(p)) {
            header.push(s);
        }
    }
    let (no_edit, rebuild) = rebuilt_re();
    header.iter().any(|s| no_edit.is_match(s)) && header.iter().any(|s| rebuild.is_match(s))
}

/// Minified/bundled output: machine-compacted code reads as extreme mean
/// line length (the TF Closure bundle averages 447 bytes/line over 519
/// lines; the densest hand-written scored file corpus-wide averages 101).
/// Threshold 200 splits the gap with ~2x margin either side; it fires on
/// layout alone, so headerless bundles are caught without bundler-name
/// lists. Joins the verdict-neutral bucket (same economics as generated:
/// regenerable output, not change targets).
pub fn is_minified(text: &str) -> bool {
    let lines = splitlines(text);
    if lines.is_empty() {
        return false;
    }
    text.len() as u64 > 200 * lines.len() as u64
}

// --- test split ---

const TEST_DIRS: &[&str] = &[
    "tests",
    "test",
    "__tests__",
    "e2e",
    "testing",
    "benches",
    "testdata",
    "test-data",
    "fixtures",
    "baselines",
    "snapshots",
    "__snapshots__",
    "examples",
    "example",
    "tutorial",
    "playwright",
    "generated",
];
const TEST_SUFFIXES: &[&str] = &[
    "_test.py",
    "_test.ts",
    "_test.js",
    "_test.tsx",
    "_test.jsx",
    ".spec.ts",
    ".spec.js",
    ".spec.tsx",
    ".test.ts",
    ".test.js",
    ".test.tsx",
    ".test.jsx",
    ".test.mjs",
    ".test.cjs",
    ".test.mts",
    ".test-d.ts",
    ".test-d.tsx",
    ".test-d.mts",
    ".test-d.js",
    ".baseline",
    ".snap",
];
const DECL_SUFFIXES: &[&str] = &[".d.ts", ".d.mts", ".d.cts", ".pyi"];

fn is_test_dir(name: &str) -> bool {
    TEST_DIRS.contains(&name)
        || name.starts_with("test-")
        || name.starts_with("test_")
        || name.ends_with("-test")
        || name.ends_with("-tests")
        || name.ends_with("_test")
        || name.ends_with("_tests")
}

pub fn is_test_file(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts[..parts.len().saturating_sub(1)]
        .iter()
        .any(|p| is_test_dir(p))
    {
        return true;
    }
    let base = parts.last().unwrap_or(&"").to_lowercase();
    base.starts_with("test_")
        || base == "conftest.py"
        || TEST_SUFFIXES
            .iter()
            .chain(DECL_SUFFIXES.iter())
            .any(|s| base.ends_with(s))
        || base.contains(".generated.")
        || base.contains(".e2e.")
        || base.contains(".e2e-")
        || base.contains("-tests.")
}

// --- clone windows ---

pub const CLONE_MIN_LINES: usize = 8;
pub const CLONE_MAX_FILE_LOC: u32 = 10000;
const CLONE_SKIP_PREFIXES: &[&str] = &[
    "#", "//", "*", "<!--", "import ", "import{", "from ", "use ", "export ", "require(",
];

/// Normalized sliding-window hashes for copy-paste detection.
/// Returns (hex digests, kept-line count).
pub fn clone_windows(text: &str) -> (Vec<String>, usize) {
    let kept: Vec<&str> = splitlines(text)
        .into_iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !CLONE_SKIP_PREFIXES.iter().any(|p| l.starts_with(p)))
        .collect();
    let mut out = Vec::new();
    if kept.len() >= CLONE_MIN_LINES {
        for i in 0..=kept.len() - CLONE_MIN_LINES {
            let digest = md5::compute(kept[i..i + CLONE_MIN_LINES].join("\n"));
            out.push(format!("{digest:x}"));
        }
    }
    (out, kept.len())
}
