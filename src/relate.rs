//! Relationship analysis: import coupling attribution, import-cycle
//! detection, and copy-paste (clone) detection.
//!
//! These are heuristic resolvers, not full module systems — every rule here
//! mirrors the reference implementation exactly (suffix matching, uniqueness
//! requirements, sort orders). Anything that disagrees with it on real
//! corpus files is a bug in this module.

use crate::scan::{FileRec, Imports};
use crate::resolve::{build_indexes, expand_rs_use, full_match, importer_dir, normalize_rel, pkg_entry_candidates, resolve_js, resolve_relative, resolve_rs_use, rust_crate_root, rust_mod_segments, rust_module_map, slash, splitext_ext, stem_of};
use std::collections::{HashMap, HashSet};
use std::path::Path;


// Submodule facades: implementation lives in coherent units; these paths
// stay stable so callers never churn on refactors.
pub use crate::clones::{CloneResult, find_clones, CLONE_MIN_LINES};
pub use crate::cycles::find_cycles;

#[derive(Debug, Default)]
pub struct Coupling {
    /// Reasoning-coupling fan-in per target (including type-only).
    pub fan_in: HashMap<String, u32>,
    /// Workspace-specifier imports per importer (internal by definition).
    pub fan_out_ws: HashMap<String, u32>,
    /// Resolved internal edges (importer, target, deferred).
    pub edges: Vec<(String, String, bool)>,
}

/// Resolve internal imports to (importer, target, deferred) edges.
pub fn attribute_coupling(
    files: &[FileRec],
    tops: &HashSet<String>,
    ws_packages: &HashMap<String, String>,
    root: &Path,
) -> Coupling {
    let idx = build_indexes(files);
    let mut out = Coupling::default();
    let empty_ws: HashMap<String, String> = HashMap::new();
    // Rust module tree, built once (only when .rs files exist).
    let has_rs = files.iter().any(|f| f.path.ends_with(".rs"));
    let (rs_mods, rs_members) = if has_rs {
        rust_module_map(files, root)
    } else {
        (HashMap::new(), HashMap::new())
    };
    for f in files {
        // Python-style imports, JS-style imports, Rust use-decls (resolved
        // onto the module tree below), or neither (parse errors).
        enum Imps<'a> {
            Py(&'a Vec<crate::py::PyImport>),
            Js(&'a Vec<String>),
            Rs(&'a Vec<String>),
            None,
        }
        let imps = match &f.imports {
            Some(Imports::Py(v)) => Imps::Py(v),
            Some(Imports::Js(v)) => Imps::Js(v),
            Some(Imports::Rs(v)) => Imps::Rs(v),
            None => Imps::None,
        };
        // Collect (module, level, type_only, deferred, is_js, js_text) uniformly.
        struct One {
            module: String,
            level: u32,
            type_only: bool,
            deferred: bool,
            js: Option<String>,
            /// Pre-resolved Rust target file (resolution needs the
            /// importer's own module, so it happens up front).
            rs: Option<String>,
        }
        let mut ones: Vec<One> = Vec::new();
        match imps {
            Imps::Py(v) => {
                for i in v {
                    ones.push(One {
                        module: i.module.clone(),
                        level: i.level,
                        type_only: i.type_only,
                        deferred: i.deferred,
                        js: None,
                        rs: None,
                    });
                }
            }
            Imps::Js(v) => {
                let to: HashSet<&str> = f
                    .type_only_imports
                    .as_ref()
                    .map(|t| t.iter().map(|s| s.as_str()).collect())
                    .unwrap_or_default();
                for s in v {
                    ones.push(One {
                        module: String::new(),
                        level: 0,
                        type_only: to.contains(s.as_str()),
                        deferred: false,
                        js: Some(s.clone()),
                        rs: None,
                    });
                }
            }
            Imps::Rs(v) => {
                let own_crate = rust_crate_root(&f.path, root);
                let own_mod = rust_mod_segments(&f.path, &own_crate);
                for s in v {
                    // One edge per DISTINCT target per use statement: a
                    // braced list naming N items from one module is one
                    // dependency, not N (the shared tail code counts one
                    // edge per import — "no inflation").
                    let mut targets: Vec<String> = Vec::new();
                    for segs in expand_rs_use(s) {
                        let (krate, full) = match resolve_rs_use(&segs, &own_crate, &own_mod, &rs_members) {
                            Some(k) => k,
                            None => continue,
                        };
                        // The tail names the imported ITEM; the target file
                        // is the longest module prefix with a unique owner
                        // (`use crate::leaf::val` → module `leaf`). Valid
                        // code cannot have both `foo::bar` (module) and an
                        // item `bar` in `foo`, so longest-first is exact.
                        let mut target = None;
                        let mut prefix = full.clone();
                        loop {
                            match rs_mods.get(&(krate.clone(), prefix.clone())) {
                                Some(hits) if hits.len() == 1 && hits[0] != f.path => {
                                    target = Some(hits[0].clone());
                                    break;
                                }
                                Some(_) => break, // ambiguous (or self): no edge
                                None => {
                                    if prefix.is_empty() {
                                        break;
                                    }
                                    prefix.pop();
                                }
                            }
                        }
                        if let Some(t) = target {
                            if !targets.contains(&t) {
                                targets.push(t);
                            }
                        }
                    }
                    for t in targets {
                        ones.push(One {
                            module: String::new(),
                            level: 0,
                            type_only: false,
                            deferred: false,
                            js: None,
                            rs: Some(t),
                        });
                    }
                }
            }
            Imps::None => continue,
        }
        let is_js_kind = f.kind == "js";
        for one in &ones {
            let cands: Vec<String> = if let Some(t) = &one.rs {
                vec![t.clone()]
            } else if let Some(imp) = &one.js {
                if imp.is_empty() {
                    continue;
                }
                let ws = if is_js_kind { ws_packages } else { &empty_ws };
                let parts: Vec<&str> = imp.split('/').collect();
                let stem = if imp.starts_with('@') && parts.len() >= 2 {
                    format!("{}/{}", parts[0], parts[1])
                } else {
                    parts[0].to_string()
                };
                if imp.starts_with('.') {
                    let mut c = resolve_js(&idx, &f.path, imp);
                    if c.is_empty() && splitext_ext(imp).is_empty() {
                        // './dir' names a directory: that dir's index.*
                        // (barrel), mirroring Python's __init__ rule.
                        let dir_key = normalize_rel(&importer_dir(&f.path), imp).join("/");
                        if let Some(hits) = idx.dir.get(&dir_key) {
                            if hits.len() == 1 {
                                c = hits.clone();
                            }
                        }
                    }
                    if c.is_empty() {
                        // Tail fallback over significant segments only
                        // ("."/"" dropped, ".." kept — like the reference).
                        let segs: Vec<&str> = imp
                            .split('/')
                            .filter(|s| *s != "." && !s.is_empty())
                            .collect();
                        let base = segs.last().map(|s| s.split('.').next().unwrap_or("")).unwrap_or("");
                        if base.is_empty() {
                            continue;
                        }
                        let key = if segs.len() >= 2 {
                            format!("{}/{}", segs[segs.len() - 2], base)
                        } else {
                            base.to_string()
                        };
                        c = idx
                            .by_tail
                            .get(&key)
                            .or_else(|| idx.by_tail.get(base))
                            .cloned()
                            .unwrap_or_default();
                    }
                    c
                } else if let Some(ws_dir) = ws.get(&stem) {
                    *out.fan_out_ws.entry(f.path.clone()).or_insert(0) += 1;
                    let rest: Vec<&str> = if imp.starts_with('@') {
                        parts[2..].to_vec()
                    } else {
                        parts[1..].to_vec()
                    };
                    let mut cands: Vec<String> = Vec::new();
                    if !rest.is_empty() {
                        let mut probe: Vec<String> = vec![ws_dir.clone()];
                        probe.extend(rest.iter().map(|s| s.to_string()));
                        let nsegs = probe.len();
                        for i in nsegs.saturating_sub(3)..nsegs {
                            if let Some(hits) = idx.suffix.get(&probe[i..].join("/")) {
                                if hits.len() == 1 {
                                    cands = hits.clone();
                                    break;
                                }
                            }
                        }
                    }
                    if cands.is_empty() {
                        // Entry fallback: manifest-declared entries first
                        // (rooted at the audit root), then index.*
                        // conventions. The entry must lie INSIDE the
                        // imported package's dir; anything still ambiguous
                        // draws no edge.
                        let ws_norm = slash(ws_dir);
                        for e in pkg_entry_candidates(&root.join(ws_dir)) {
                            let stem_e = stem_of(&e).to_string();
                            let hits: &[String] = if stem_e.contains('/') {
                                idx.suffix.get(&stem_e).map(|v| v.as_slice()).unwrap_or(&[])
                            } else {
                                idx.by_tail.get(&stem_e).map(|v| v.as_slice()).unwrap_or(&[])
                            };
                            let inside: Vec<String> = hits
                                .iter()
                                .filter(|h| slash(h).starts_with(&format!("{ws_norm}/")))
                                .cloned()
                                .collect();
                            if inside.len() == 1 && inside[0] != f.path {
                                cands = inside;
                                break;
                            }
                        }
                    }
                    cands
                } else {
                    continue;
                }
            } else {
                if one.module.is_empty() && one.level == 0 {
                    continue;
                }
                if one.level == 0
                    && !tops.contains(one.module.split('.').next().unwrap_or(""))
                {
                    continue; // external absolute import
                }
                let mut c = if one.level > 0 {
                    resolve_relative(&idx, &f.path, one.level, &one.module)
                } else {
                    full_match(&idx, &one.module.split('.').map(|s| s.to_string()).collect::<Vec<_>>())
                };
                if c.is_empty() {
                    // Bare top-level import (`import pkg` from inside pkg):
                    // resolve the package root, else the same-named module —
                    // never an unrelated same-stem file elsewhere in the
                    // tree (that phantom once welded
                    // django/utils/warnings.py into a 44-module cycle via
                    // template/backends/django.py). Package root wins when
                    // both exist: importing a package always executes its
                    // __init__. Ambiguity drops the edge instead of guessing.
                    let tparts: Vec<&str> = one.module.split('.').collect();
                    if tparts.len() == 1 && tops.contains(tparts[0]) {
                        let seg = tparts[0];
                        if let Some(hits) = idx.pkg_init.get(seg) {
                            if hits.len() == 1 {
                                c = hits.clone();
                            }
                        } else if let Some(hits) = idx.by_tail.get(seg) {
                            if hits.len() == 1 {
                                c = hits.clone();
                            }
                        }
                    } else {
                        // Tail fallback, most-specific key first.
                        let tail = &tparts[tparts.len().saturating_sub(2)..];
                        let key = tail.join("/");
                        c = idx
                            .by_tail
                            .get(&key)
                            .or_else(|| idx.by_tail.get(*tail.last().unwrap_or(&"")))
                            .cloned()
                            .unwrap_or_default();
                    }
                }
                c
            };
            // sorted (not set) order: set iteration would depend on hash
            // seed, making attribution nondeterministic across runs.
            let mut ordered = cands;
            ordered.sort();
            ordered.dedup();
            for c in ordered {
                if c != f.path {
                    if !one.type_only {
                        // Runtime coupling only: annotation-only references
                        // (`if TYPE_CHECKING:`, `import type`) never break at
                        // runtime, so they draw no ripple claim — same rule
                        // as cycles and focus blast radius, which already
                        // exclude them. fan_in now agrees with edges exactly.
                        *out.fan_in.entry(c.clone()).or_insert(0) += 1;
                        out.edges.push((f.path.clone(), c, one.deferred));
                    }
                    break; // one target per import, no inflation
                }
            }
        }
    }
    out
}
