//! Proposed-change evaluation harness: Exp 1 BM25, Exp 2 graph expansion,
//! and end-to-end --intent reporting checks over synthetic mini-repos.

mod common;

use cxcap::dispatch;
use cxcap::lexical;
use cxcap::scan;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(10_000);

struct Tmp {
    path: PathBuf,
}

impl Tmp {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("cxcap-intent-{}-{}", std::process::id(), id));
        fs::create_dir_all(&p).unwrap();
        Tmp { path: p }
    }
    fn write(&self, rel: &str, content: &str) {
        let p = self.path.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn scan_repo(root: &std::path::Path) -> Vec<scan::FileRec> {
    let mut entries = Vec::new();
    let mut symlinks = 0u64;
    scan::iter_files(root, &mut entries, &mut symlinks);
    entries.sort();
    let tops = dispatch::local_toplevels(root);
    entries
        .iter()
        .map(|e| scan::scan_one(e, 2_000_000, &tops).rec)
        .collect()
}

fn coupling_edges(root: &std::path::Path, files: &[scan::FileRec]) -> Vec<(String, String)> {
    let tops = dispatch::local_toplevels(root);
    let c = cxcap::relate::attribute_coupling(files, &tops, &std::collections::HashMap::new(), root);
    c.edges.into_iter().map(|(a, b, _)| (a, b)).collect()
}

fn fixture_repo() -> Tmp {
    let t = Tmp::new();
    t.write(
        "auth/session.ts",
        "export interface SessionConfig { token: string; }\nexport function createSession(c: SessionConfig) { return c; }\n",
    );
    t.write(
        "auth/service.ts",
        "import { SessionConfig } from './session';\nexport class AuthService { cfg: SessionConfig | null = null; }\n",
    );
    t.write("util/string.ts", "export function padLeft(s: string) { return s; }\n");
    t.write("docs/auth.md", "# Session\n\nThe session system handles logins.\n");
    t.write(
        "auth/session.test.ts",
        "import { createSession } from './session';\ntest('session works', () => { createSession({token:'x'}); });\n",
    );
    t.write(
        "generated/bundle.js",
        "// GENERATED FILE - do not edit, rebuild with make\n// session session session\nexport const x = 1;\n",
    );
    t
}

#[test]
fn intent_finds_symbol_not_just_path() {
    let t = fixture_repo();
    let files = scan_repo(&t.path);
    let ranked: Vec<String> = lexical::rank("session config", &files, 5)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert!(!ranked.is_empty());
    assert_eq!(ranked[0], "auth/session.ts");
}

#[test]
fn test_and_generated_surfaces_excluded() {
    let t = fixture_repo();
    let files = scan_repo(&t.path);
    let ranked: Vec<String> =
        lexical::rank("session", &files, 10).into_iter().map(|(p, _)| p).collect();
    assert!(!ranked.contains(&"auth/session.test.ts".to_string()));
    assert!(!ranked.contains(&"generated/bundle.js".to_string()));
    assert!(!ranked.contains(&"docs/auth.md".to_string()));
    assert!(ranked.contains(&"auth/session.ts".to_string()));
}

#[test]
fn bm25_matches_or_beats_literal_on_fixture() {
    let t = fixture_repo();
    let files = scan_repo(&t.path);
    let actual = vec!["auth/session.ts".to_string()];
    let b: Vec<String> = lexical::rank("session config", &files, 3)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    let l: Vec<String> = common::rank_literal("session config", &files, 3)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert!(common::recall_at_k(&b, &actual, 1) >= common::recall_at_k(&l, &actual, 1));
}

#[test]
fn real_repo_cycle_intent_hits_relate() {
    // Read-only scan of CXCAP's own src (parent-state proxy).
    let root = common::pkg_root().join("src");
    let files = scan_repo(&root);
    assert!(!files.is_empty());
    let ranked: Vec<String> = lexical::rank("import cycle detection", &files, 3)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert!(
        ranked.iter().any(|p| p.ends_with("relate.rs") || p.ends_with("cycles.rs")),
        "expected cycles.rs in top-3 for cycle intent, got {ranked:?}"
    );
}

#[test]
fn real_repo_workspace_intent_hits_dispatch() {
    let root = common::pkg_root().join("src");
    let files = scan_repo(&root);
    let ranked: Vec<String> = lexical::rank("workspace package resolution", &files, 3)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    // Ranking follows the code: package-root logic lives in packages.rs
    // (split out of dispatch.rs); either location proves localization works.
    assert!(
        ranked.iter().any(|p| p.ends_with("packages.rs") || p.ends_with("dispatch.rs")),
        "expected packages.rs in top-3 for workspace intent, got {ranked:?}"
    );
}

#[test]
fn determinism_same_intent_same_order() {
    let t = fixture_repo();
    let files = scan_repo(&t.path);
    let a: Vec<String> =
        lexical::rank("session", &files, 5).into_iter().map(|(p, _)| p).collect();
    let b: Vec<String> =
        lexical::rank("session", &files, 5).into_iter().map(|(p, _)| p).collect();
    assert_eq!(a, b);
    let _empty: HashSet<String> = HashSet::new();
}

#[test]
fn graph_expansion_recovers_dependent_lexical_misses() {
    // Intent names the config; the service file shares only the 'auth'
    // path token and may not rank. Expansion along the real import edge
    // (service -> session) must recover it without pulling util/.
    let t = fixture_repo();
    let files = scan_repo(&t.path);
    let edges = coupling_edges(&t.path, &files);
    assert!(
        edges.iter().any(|(a, b)| a == "auth/service.ts" && b == "auth/session.ts"),
        "fixture edge missing: {edges:?}"
    );
    let seeds: Vec<String> = lexical::rank("session config", &files, 1)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert_eq!(seeds, vec!["auth/session.ts".to_string()]);
    let expanded = lexical::expand_seeds(&seeds, &edges, 1, 20);
    assert!(expanded.contains(&"auth/service.ts".to_string()));
    assert!(!expanded.contains(&"util/string.ts".to_string()));
    // Second-order context is strictly larger than lexical-only.
    let actual = vec!["auth/session.ts".to_string(), "auth/service.ts".to_string()];
    assert!(common::recall_at_k(&expanded, &actual, 10) > common::recall_at_k(&seeds, &actual, 10));
}

#[test]
fn graph_expansion_does_not_swallow_repo_on_generic_word() {
    // Generic 'auth' matches two files; expansion must stay bounded and
    // must not drag in the isolated util file at 1 hop... but service <->
    // session are linked, so the closed pair is the max.
    let t = fixture_repo();
    let files = scan_repo(&t.path);
    let edges = coupling_edges(&t.path, &files);
    let seeds: Vec<String> =
        lexical::rank("auth", &files, 5).into_iter().map(|(p, _)| p).collect();
    let expanded = lexical::expand_seeds(&seeds, &edges, 1, 5);
    assert!(expanded.len() <= 5);
    assert!(!expanded.contains(&"util/string.ts".to_string()));
}

fn audit_json(dir: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let bin = common::bin();
    let out = std::process::Command::new(bin)
        .arg("audit")
        .arg(dir)
        .arg("--json")
        .args(args)
        .output()
        .expect("run cxcap binary");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON")
}

#[test]
fn intent_cli_json_block() {
    let t = fixture_repo();
    let rep = audit_json(&t.path, &["--intent", "session config"]);
    let it = rep.get("intent").expect("intent block present");
    assert_eq!(it.get("query").and_then(|q| q.as_str()), Some("session config"));
    let seeds: Vec<String> = it
        .get("seeds")
        .and_then(|s| s.as_array())
        .map(|a| a.iter().filter_map(|s| s.get("path").and_then(|p| p.as_str()).map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(!seeds.is_empty());
    assert_eq!(seeds[0], "auth/session.ts");
    let expanded: Vec<String> = it
        .get("expanded")
        .and_then(|e| e.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect())
        .unwrap_or_default();
    assert!(expanded.contains(&"auth/service.ts".to_string()));
    assert!(!expanded.contains(&"util/string.ts".to_string()));
    let ctx: Vec<String> = it
        .get("context_lines")
        .and_then(|e| e.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect())
        .unwrap_or_default();
    assert!(ctx.len() >= 3);
    // No legacy identity keys anywhere in the report.
    let all = serde_json::to_string(&rep).unwrap();
    assert!(!all.contains("ccc_version"));
    assert!(rep.get("cxcap_version").is_some());
}

#[test]
fn intent_cli_jobs_deterministic() {
    let t = fixture_repo();
    let a = audit_json(&t.path, &["--intent", "session", "--jobs", "1"]);
    let b = audit_json(&t.path, &["--intent", "session", "--jobs", "2"]);
    assert_eq!(a.get("intent"), b.get("intent"));
}

#[test]
fn intent_cli_text_section() {
    let t = fixture_repo();
    let bin = common::bin();
    let out = std::process::Command::new(bin)
        .arg("audit")
        .arg(&t.path)
        .arg("--intent")
        .arg("session config")
        .output()
        .expect("run cxcap binary");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("CHANGE EXPOSURE"));
    assert!(text.contains("LIKELY TOUCHPOINTS"));
    assert!(text.contains("CONTEXT SURFACE"));
    assert!(text.contains("auth/session.ts"));
}

#[test]
fn intent_cli_empty_vocab_falls_back() {
    let t = fixture_repo();
    let rep = audit_json(&t.path, &["--intent", "zzzqqq"]);
    let it = rep.get("intent").expect("intent block present");
    assert!(it.get("seeds").and_then(|s| s.as_array()).map(|a| a.is_empty()).unwrap_or(false));
    let assess = it.get("assessment").and_then(|a| a.as_str()).unwrap_or("");
    assert!(assess.contains("--focus"));
}

#[test]
fn intent_cli_uncertainty_reread() {
    let t = Tmp::new();
    t.write("util/loader.ts", "export async function loadPlugin(name: string) {\n  return await import(name);\n}\n");
    t.write("util/pad.ts", "export function pad(s: string) { return s; }\n");
    let rep = audit_json(&t.path, &["--intent", "loader plugin"]);
    let it = rep.get("intent").expect("intent block present");
    let unc = it.get("uncertainty").and_then(|u| u.as_array()).cloned().unwrap_or_default();
    assert!(
        unc.iter().any(|u| u.get("path").and_then(|p| p.as_str()) == Some("util/loader.ts")),
        "expected dynamic-import flag, got {unc:?}"
    );
}

#[test]
fn no_intent_flag_leaves_report_untouched() {
    // Default output must not contain an intent block: existing consumers
    // and snapshots see byte-equivalent shape (null field, no sections).
    let t = fixture_repo();
    let rep = audit_json(&t.path, &[]);
    assert!(rep.get("intent").map(|v| v.is_null()).unwrap_or(false));
}

#[test]
fn real_analyzer_names_untruncated() {
    // Eight helpers in one file: reporting keeps top-5 details, but the
    // ephemeral ranking source must retain every name (regression: the
    // analyzers once truncated func_details before dispatch saw them,
    // so only top-5 symbols were searchable on real repos).
    let t = Tmp::new();
    let mut src = String::new();
    for i in 0..8 {
        src.push_str(&format!("export function helper{i}() {{ return {i}; }}\n"));
    }
    t.write("util/many.ts", &src);
    let files = scan_repo(&t.path);
    let rec = files.iter().find(|f| f.path == "util/many.ts").unwrap();
    assert_eq!(rec.func_names.as_ref().map(|n| n.len()), Some(8));
    let ranked: Vec<String> =
        lexical::rank("helper7", &files, 3).into_iter().map(|(p, _)| p).collect();
    assert_eq!(ranked[0], "util/many.ts");
}

#[test]
fn intent_tangles_describe_the_context_set_only() {
    // pkga <-> pkgb is tangled repo-wide, but the intent only reaches one
    // standalone pkga file: no tangle, and singular counts.
    let t = Tmp::new();
    t.write("pkga/xylophone.ts", "export function playXylophone() { return 1; }\n");
    t.write("pkga/x.ts", "import { y } from '../pkgb/y';\nexport const x = y;\n");
    t.write("pkga/z.ts", "export const z = 2;\n");
    t.write("pkgb/y.ts", "import { z } from '../pkga/z';\nexport const y = z;\n");
    let rep = audit_json(&t.path, &["--intent", "xylophone"]);
    let it = &rep["intent"];
    let expanded: Vec<&str> = it["expanded"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(expanded, vec!["pkga/xylophone.ts"]);
    assert!(it["component_tangles"].as_array().unwrap().is_empty(), "{}", it["component_tangles"]);
    let ctx: Vec<&str> = it["context_lines"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert!(ctx.iter().any(|l| l.starts_with("1 component, 0 cross-boundary edges, 0 cycles")), "{ctx:?}");
}
