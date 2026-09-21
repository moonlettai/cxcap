//! Import resolution: mapping import statements onto repo files.
//!
//! Heuristic resolvers, not a full module system — repository-relative
//! resolution, package `__init__` handling, workspace names, Rust module
//! trees. Every rule mirrors the reference exactly; anything disagreeing
//! with it on real corpus files is a bug here.

use crate::scan::FileRec;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// os.path.splitext semantics: leading dots of the basename never start an
/// extension (`..bashrc` and `..` have none); otherwise the last dot does
/// (`a..b` -> stem `a.`, ext `.b`). Returns (stem, ext).
fn split_ext(base: &str) -> (&str, &str) {
    let stripped = base.trim_start_matches('.');
    let leading = base.len() - stripped.len();
    match stripped.rfind('.') {
        Some(rel) => {
            let i = leading + rel;
            (&base[..i], &base[i..])
        }
        None => (base, ""),
    }
}

/// Stem of a basename (the part before the extension).
pub(crate) fn stem_of(base: &str) -> &str {
    split_ext(base).0
}

/// Extension (with dot) of a path's basename, for the no-extension retry.
pub(crate) fn splitext_ext(s: &str) -> &str {
    split_ext(s.rsplit('/').next().unwrap_or(s)).1
}

pub(crate) fn slash(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn importer_dir(path: &str) -> Vec<String> {
    let p = slash(path);
    let parts: Vec<&str> = p.split('/').collect();
    parts[..parts.len().saturating_sub(1)]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

pub(crate) struct Indexes {
    pub(crate) by_tail: HashMap<String, Vec<String>>,
    pub(crate) suffix: HashMap<String, Vec<String>>,
    pub(crate) pkg_init: HashMap<String, Vec<String>>,
    pub(crate) dir: HashMap<String, Vec<String>>,
}

/// Rust module tree: (crate_root_rel, module_segments) -> declaring files.
/// Module paths from layout: a `src/` prefix is stripped;
/// `main.rs`/`lib.rs`/`mod.rs` name the parent.
pub(crate) fn rust_crate_root(file_rel: &str, root: &Path) -> String {
    // Nearest ancestor (incl. the file's own dir) with Cargo.toml.
    let mut dir = importer_dir(file_rel);
    loop {
        let cand = if dir.is_empty() {
            root.to_path_buf()
        } else {
            root.join(dir.join("/"))
        };
        if cand.join("Cargo.toml").is_file() {
            return dir.join("/");
        }
        if dir.is_empty() {
            return String::new();
        }
        dir.pop();
    }
}

pub(crate) fn rust_mod_segments(file_rel: &str, crate_rel: &str) -> Vec<String> {
    let mut segs: Vec<String> = if crate_rel.is_empty() {
        file_rel.split('/').map(|s| s.to_string()).collect()
    } else {
        match file_rel.strip_prefix(&format!("{crate_rel}/")) {
            Some(r) => r.split('/').map(|s| s.to_string()).collect(),
            None => return Vec::new(),
        }
    };
    if segs.first().map(|s| s == "src").unwrap_or(false) {
        segs.remove(0);
    }
    if let Some(last) = segs.pop() {
        let stem = last.strip_suffix(".rs").unwrap_or(&last);
        if !matches!(stem, "main" | "lib" | "mod") {
            segs.push(stem.to_string());
        }
    }
    segs
}

pub(crate) fn rust_module_map(
    files: &[FileRec],
    root: &Path,
) -> (HashMap<(String, Vec<String>), Vec<String>>, HashMap<String, String>) {
    let mut map: HashMap<(String, Vec<String>), Vec<String>> = HashMap::new();
    let mut members: HashMap<String, String> = HashMap::new();
    for f in files {
        if !f.path.ends_with(".rs") {
            continue;
        }
        let crate_rel = rust_crate_root(&f.path, root);
        if !crate_rel.is_empty() {
            // Workspace-member shorthand: package name ≈ member dir name.
            // Used only to resolve extern-crate heads to member roots;
            // mismatches silently miss (never false edges).
            if let Some(name) = crate_rel.rsplit('/').next() {
                members.entry(name.to_string()).or_insert(crate_rel.clone());
            }
        }
        let segs = rust_mod_segments(&f.path, &crate_rel);
        map.entry((crate_rel, segs)).or_default().push(f.path.clone());
    }
    (map, members)
}

/// Expand one `use` path into concrete module paths: brace lists multiply
/// (`a::{b, c}` → `a::b`, `a::c`, nesting-aware), ` as alias` is dropped
/// (aliases don't change the target), trailing `::*` names the module
/// itself (glob import = edge to the module, mirroring `from x import *`).
pub(crate) fn expand_rs_use(path: &str) -> Vec<Vec<String>> {
    /// Split `::` at brace depth 0 (a naive split fractures `{a::b}`).
    fn split_path(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut depth = 0i32;
        let mut start = 0;
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                b':' if depth == 0 && b.get(i + 1) == Some(&b':') => {
                    out.push(s[start..i].to_string());
                    i += 1;
                    start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        out.push(s[start..].to_string());
        out
    }
    fn split_top(s: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let (mut a, mut p, mut br, mut c) = (0i32, 0i32, 0i32, 0i32);
        let mut start = 0;
        for (i, ch) in s.char_indices() {
            match ch {
                '<' => a += 1,
                '>' => a -= 1,
                '(' => p += 1,
                ')' => p -= 1,
                '[' => br += 1,
                ']' => br -= 1,
                '{' => c += 1,
                '}' => c -= 1,
                ',' if a == 0 && p == 0 && br == 0 && c == 0 => {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        out.push(&s[start..]);
        out
    }
    fn expand(segs: &[String], out: &mut Vec<Vec<String>>, depth: u32) {
        if depth > 8 {
            return; // pathological nesting: miss rather than hang
        }
        if let Some(i) = segs.iter().position(|s| s.contains('{')) {
            let (open, rest) = segs[i].split_once('{').unwrap_or((segs[i].as_str(), ""));
            let inner = rest
                .strip_prefix('{')
                .unwrap_or(rest)
                .strip_suffix('}')
                .unwrap_or(rest);
            for part in split_top(inner) {
                let mut v: Vec<String> = segs[..i].to_vec();
                v.extend(open.split("::").filter(|s| !s.is_empty()).map(|s| s.to_string()));
                // NB: split_path, not split — parts can nest (`b::{c,d}`).
                v.extend(split_path(part));
                expand(&v, out, depth + 1);
            }
            return;
        }
        out.push(segs.to_vec());
    }
    let mut expanded = Vec::new();
    expand(&split_path(path), &mut expanded, 0);
    expanded
        .into_iter()
        .map(|mut segs| {
            // `a as b` → `a`; trailing `*` → the module itself; a lone
            // `self` segment likewise names its own path.
            if let Some(last) = segs.pop() {
                let clean = last.split(" as ").next().unwrap_or(&last).to_string();
                if clean == "*" || clean == "self" {
                    // keep segs as-is (module itself)
                } else {
                    segs.push(clean);
                }
            }
            segs.into_iter().filter(|s| !s.is_empty()).collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .collect()
}

/// Resolve expanded `use` segments to (crate_rel, module) or None
/// (external crate, or a shape with no unique meaning).
pub(crate) fn resolve_rs_use(
    segs: &[String],
    own_crate: &str,
    own_mod: &[String],
    members: &HashMap<String, String>,
) -> Option<(String, Vec<String>)> {
    if segs.is_empty() {
        return None;
    }
    let mut i = 0;
    let (krate, base): (String, Vec<String>) = match segs[0].as_str() {
        "crate" => {
            i = 1;
            (own_crate.to_string(), Vec::new())
        }
        "self" => {
            i = 1;
            (own_crate.to_string(), own_mod.to_vec())
        }
        "super" => {
            let mut m = own_mod.to_vec();
            while i < segs.len() && segs[i] == "super" {
                m.pop();
                i += 1;
            }
            (own_crate.to_string(), m)
        }
        head => {
            // 2018+ paths: a bare head names an extern crate. Workspace
            // members resolve by directory name; anything else is external.
            let root = members.get(head)?.clone();
            (root, Vec::new())
        }
    };
    let mut path = base;
    for s in &segs[i..] {
        if s == "super" || s == "self" || s == "*" {
            // Mid-path super/self after the head (e.g. `crate::a::super`)
            // has no module meaning here: give up rather than guess.
            return None;
        }
        path.push(s.clone());
    }
    Some((krate, path))
}

pub(crate) fn build_indexes(files: &[FileRec]) -> Indexes {
    let mut idx = Indexes {
        by_tail: HashMap::new(),
        suffix: HashMap::new(),
        pkg_init: HashMap::new(),
        dir: HashMap::new(),
    };
    for f in files {
        let p = slash(&f.path);
        let parts: Vec<&str> = p.split('/').collect();
        let stem = stem_of(parts[parts.len() - 1]).to_string();
        idx.by_tail.entry(stem.clone()).or_default().push(f.path.clone());
        if parts.len() >= 2 {
            idx.by_tail
                .entry(format!("{}/{}", parts[parts.len() - 2], stem))
                .or_default()
                .push(f.path.clone());
        }
        let mut noext: Vec<&str> = parts[..parts.len() - 1].to_vec();
        noext.push(&stem);
        // Leak-free join: build owned segments for the loop below.
        let noext: Vec<String> = noext.iter().map(|s| s.to_string()).collect();
        for i in 0..noext.len().saturating_sub(1) {
            idx.suffix
                .entry(noext[i..].join("/"))
                .or_default()
                .push(f.path.clone());
        }
        if stem == "__init__" && parts.len() >= 2 {
            let d = &parts[..parts.len() - 1];
            for i in 0..d.len() {
                idx.pkg_init
                    .entry(d[i..].join("/"))
                    .or_default()
                    .push(f.path.clone());
            }
        }
        if stem == "index" && parts.len() >= 2 {
            idx.dir
                .entry(parts[..parts.len() - 1].join("/"))
                .or_default()
                .push(f.path.clone());
        }
    }
    idx
}

/// Unique repo-relative resolution of a dotted/dir path. Package dirs
/// resolve to their __init__. Returns the single path or nothing.
pub(crate) fn full_match(idx: &Indexes, segs: &[String]) -> Vec<String> {
    if segs.len() < 2 {
        return Vec::new();
    }
    let key = segs.join("/");
    if let Some(hits) = idx.suffix.get(&key) {
        if hits.len() == 1 {
            return hits.clone();
        }
    }
    if let Some(pkg) = idx.pkg_init.get(&key) {
        if pkg.len() == 1 {
            return pkg.clone();
        }
    }
    Vec::new()
}

/// Lexically normalize a relative JS path against the importer dir. The
/// last segment is extension-stripped at the LAST dot — matching how the
/// indexes are keyed — so compound extensions resolve to their true files
/// (`cache.svelte.js` → `cache.svelte`, not `cache`; the old first-dot
/// rule fell through to the tail fallback and drew phantom edges between
/// same-stem files).
pub(crate) fn normalize_rel(base: &[String], imp: &str) -> Vec<String> {
    let mut out = base.to_vec();
    let segs: Vec<&str> = imp.split('/').collect();
    let last = segs.last().copied().unwrap_or("");
    for s in segs {
        if s.is_empty() || s == "." {
            continue;
        }
        if s == ".." {
            out.pop();
        } else if s == last {
            out.push(split_ext(s).0.to_string());
        } else {
            out.push(s.to_string());
        }
    }
    out
}

pub(crate) fn resolve_relative(idx: &Indexes, path: &str, level: u32, module: &str) -> Vec<String> {
    let mut base = importer_dir(path);
    if level > 1 {
        let cut = level as usize - 1;
        let n = base.len();
        base.truncate(n.saturating_sub(cut));
    }
    if module.is_empty() {
        full_match(idx, &base)
    } else {
        base.extend(module.split('.').map(|s| s.to_string()));
        full_match(idx, &base)
    }
}

pub(crate) fn resolve_js(idx: &Indexes, path: &str, imp: &str) -> Vec<String> {
    full_match(idx, &normalize_rel(&importer_dir(path), imp))
}

pub(crate) fn read_json_strict(path: &Path) -> Option<serde_json::Value> {
    // utf-8-sig with strict decoding: undecodable manifests are skipped,
    // like the reference (OSError/ValueError both continue). Single
    // contract shared with the workspace scan in packages.rs.
    let b = fs::read(path).ok()?;
    let b = b
        .strip_prefix(b"\xef\xbb\xbf")
        .map(|s| s.to_vec())
        .unwrap_or(b);
    let s = std::str::from_utf8(&b).ok()?;
    serde_json::from_str(s).ok()
}

fn json_truthy(v: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match v {
        Null => false,
        Bool(b) => *b,
        Number(n) => n.as_f64() != Some(0.0),
        String(s) => !s.is_empty(),
        Array(a) => !a.is_empty(),
        Object(o) => !o.is_empty(),
    }
}

/// Strip a bundler entry extension (`\.(m|c)?js$|\.jsx$|\.ts$`, leftmost
/// match, `$` also matching before a single trailing newline).
///
/// All alternatives are end-anchored and pairwise non-interfering (no one
/// is a dotted suffix of another: `.mjs`/`.cjs` never end in `.js`, and
/// nothing ending in `.jsx` ends in `.js`), so the leftmost match always
/// coincides with stripping a single trailing suffix — expressed directly
/// with std matching instead of a byte scanner. The swallowed newline goes
/// with the suffix (a lone trailing `\n` is transparent to `$`).
pub(crate) fn strip_entry_ext(s: &str) -> String {
    // `$`-before-newline: match against the body, drop the newline with
    // the suffix. Exactly one trailing newline is transparent; two (or a
    // suffix that needs the newline) means no match, like the reference.
    let body = match s.strip_suffix('\n') {
        Some(b) if !b.ends_with('\n') => b,
        _ => s,
    };
    for suffix in [".mjs", ".cjs", ".js", ".jsx", ".ts"] {
        if let Some(base) = body.strip_suffix(suffix) {
            return base.to_string();
        }
    }
    s.to_string()
}

/// Possible entry files a package may expose, manifest-declared first.
pub fn pkg_entry_candidates(pkg_dir: &Path) -> Vec<String> {
    let out = [
        "index.ts",
        "index.tsx",
        "index.mts",
        "index.js",
        "index.jsx",
        "index.mjs",
        "index.cjs",
    ];
    let owned: Vec<String> = out.iter().map(|s| s.to_string()).collect();
    let v = match read_json_strict(&pkg_dir.join("package.json")) {
        Some(v) => v,
        None => return owned,
    };
    let d = match v.as_object() {
        Some(d) => d,
        None => return owned,
    };
    // `d.get("module") or d.get("main")`: first truthy value wins.
    let main_val = match d.get("module") {
        Some(v) if json_truthy(v) => Some(v),
        _ => d.get("main"),
    };
    let main = match main_val {
        Some(serde_json::Value::String(s)) => s,
        _ => return owned,
    };
    let base = strip_entry_ext(&main.replace('\\', "/"));
    if base.is_empty() {
        return owned;
    }
    let mut res: Vec<String> = [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]
        .iter()
        .map(|e| format!("{base}{e}"))
        .collect();
    res.extend(owned);
    res
}

#[cfg(test)]
mod tests {
    use super::strip_entry_ext;

    fn stripped(cases: &[(&str, &str)]) {
        for (input, want) in cases {
            assert_eq!(&strip_entry_ext(input), want, "input {input:?}");
        }
    }

    #[test]
    fn strips_known_bundler_suffixes() {
        stripped(&[
            ("dist/main.js", "dist/main"),
            ("dist/main.mjs", "dist/main"),
            ("dist/main.cjs", "dist/main"),
            ("dist/main.jsx", "dist/main"),
            ("src/i.ts", "src/i"),
            ("pkg/dist/main.mjs", "pkg/dist/main"),
            (".js", ""),
        ]);
    }

    #[test]
    fn leaves_unknown_or_missing_suffixes() {
        stripped(&[
            ("index", "index"),
            ("a.mts", "a.mts"),
            ("a.tsx", "a.tsx"),
            ("a.JS", "a.JS"),
            ("a.cjsx", "a.cjsx"),
            ("", ""),
            (".", "."),
        ]);
    }

    #[test]
    fn chained_suffixes_strip_once_from_the_left() {
        stripped(&[
            ("a.ts.js", "a.ts"),
            ("a.js.ts", "a.js"),
            ("a.jsx.js", "a.jsx"),
            ("a..js", "a."),
        ]);
    }

    #[test]
    fn dollar_matches_before_a_single_trailing_newline() {
        stripped(&[
            ("dist/main.js\n", "dist/main"),
            ("a.ts\n", "a"),
            ("a.js\n\n", "a.js\n\n"),
            ("a.js ", "a.js "),
            ("\n", "\n"),
        ]);
    }
}
