//! CXCAP regression suite: CLI behavior over synthetic fixtures.
//!
//! Mirrors the pre-cutover Python suite section-for-section (adapted where
//! the port deliberately differs), plus locks for discoveries made during
//! the port: CPython-verbatim error messages, PEP 696 defaults, canonical
//! determinism across worker counts. Runs the real binary via
//! The real binary, located relatively — no mocks, no stubs.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// Unique scratch dir, removed on drop (best effort).
struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "cxcap-rs-test-{}-{}",
            std::process::id(),
            id
        ));
        fs::create_dir_all(&path).unwrap();
        TestDir { path }
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.path.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
    }

    fn write_bytes(&self, rel: &str, content: &[u8]) {
        let p = self.path.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
    }

    fn sub(&self, rel: &str) -> PathBuf {
        self.path.join(rel)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

mod common;

fn bin() -> PathBuf {
    common::bin()
}

fn audit(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.arg("audit").arg(dir).args(args);
    let out = cmd.output().expect("run cxcap binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn audit_json(dir: &Path, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend(args);
    let (code, stdout, stderr) = audit(dir, &full);
    assert_eq!(code, 0, "audit failed: {stderr}");
    serde_json::from_str(&stdout).expect("valid JSON report")
}

fn hotspots(rep: &Value) -> std::collections::HashMap<String, Value> {
    rep["hotspots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| (h["path"].as_str().unwrap().to_string(), h.clone()))
        .collect()
}

fn get<'a>(m: &'a std::collections::HashMap<String, Value>, p: &str) -> &'a Value {
    m.get(p).unwrap_or_else(|| panic!("missing hotspot {p} in {m:?}"))
}

// 1. Python AST complexity is exact; TS heuristic sane.
#[test]
fn py_ts_complexity_exact() {
    let t = TestDir::new();
    t.write(
        "proj/a.py",
        "def plain():\n    return 1\n\ndef two_branches(x):\n    if x:\n        return 1\n    for i in range(3):\n        print(i)\n    return 0\n",
    );
    t.write(
        "proj/b.ts",
        "export function f(x: number) {\n  if (x > 1) { return 1; }\n  return 0;\n}\n",
    );
    let rep = audit_json(&t.sub("proj"), &["--top", "5"]);
    assert_eq!(rep["code_files"], 2);
    let h = hotspots(&rep);
    // plain=1, two_branches=1+if+for=3, module +1 => 5.
    assert_eq!(get(&h, "a.py")["complexity"], 5);
    assert_eq!(get(&h, "a.py")["max_func_cx"], 3);
    assert!(get(&h, "b.ts")["complexity"].as_u64().unwrap() >= 3);
}

// 2. Focus answers the expensive-area question.
#[test]
fn focus_matches() {
    let t = TestDir::new();
    t.write("proj/a.py", "def a():\n    return 1\n");
    let rep = audit_json(&t.sub("proj"), &["--focus", "a.py"]);
    let f = &rep["focus"];
    assert_eq!(f["files"], 1);
    assert!(f.get("assessment").is_some(), "{f}");
}

// 3. Read-only: mtimes unchanged, and the sources never open for write.
#[test]
fn target_untouched_and_no_write_opens() {
    let t = TestDir::new();
    t.write("proj/a.py", "def a():\n    return 1\n");
    t.write("proj/b.ts", "export const x = 1;\n");
    let mut before = std::collections::HashMap::new();
    for entry in walk_files(&t.sub("proj")) {
        before.insert(entry.clone(), fs::metadata(&entry).unwrap().modified().unwrap());
    }
    let (code, _, _) = audit(&t.sub("proj"), &[]);
    assert_eq!(code, 0);
    for (p, m) in &before {
        assert_eq!(&fs::metadata(p).unwrap().modified().unwrap(), m, "{p:?} modified");
    }
    let manifest = common::pkg_root();
    for f in ["src/main.rs", "src/scan.rs", "src/dispatch.rs", "src/py.rs",
              "src/relate.rs", "src/report.rs", "src/rs.rs", "src/ts.rs", "src/lib.rs"] {
        let src = fs::read_to_string(manifest.join(f)).unwrap();
        for pat in ["fs::write", "File::create", "OpenOptions", "remove_file", "remove_dir",
                    "create_dir", "fs::rename", "set_permissions", "::symlink", "Command::new"] {
            assert!(!src.contains(pat), "{f} contains write-capable call {pat}");
        }
    }
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

// 5. SFC: frontmatter + <script> scored, markup prose ignored.
#[test]
fn sfc_script_scored() {
    let t = TestDir::new();
    t.write(
        "sfc/Card.astro",
        "---\nimport Base from './Base.astro';\nconst x = 1;\nif (x > 0) { console.log(x); }\n---\n<div>for example just prose here</div>\n<script>function hello() { if (true) { return 1; } return 0; }</script>\n",
    );
    t.write(
        "sfc/Plain.astro",
        "---\n---\n<div>for while if prose only, no logic</div>\n",
    );
    let rep = audit_json(&t.sub("sfc"), &["--top", "5"]);
    assert_eq!(rep["code_files"], 2);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "Card.astro")["complexity"], 4);
    assert_eq!(get(&h, "Plain.astro")["complexity"], 0);
}

// 6. JS relatives create fan-in edges; bare specifiers stay external.
#[test]
fn js_relative_edges() {
    let t = TestDir::new();
    t.write("jsp/util.ts", "export function double(x: number) { return x * 2; }\n");
    t.write(
        "jsp/app.ts",
        "import { double } from './util';\nimport * as fs from 'fs';\nexport function run(x: number) { if (x > 0) { return double(x); } return 0; }\n",
    );
    let rep = audit_json(&t.sub("jsp"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "util.ts")["fan_in"], 1);
    assert_eq!(get(&h, "app.ts")["fan_out_in"], 1);
    assert_eq!(get(&h, "app.ts")["fan_out_ex"], 1);
}

// 7. No scorable code: N/A verdict + unscored listed.
#[test]
fn na_verdict() {
    let t = TestDir::new();
    t.write("static/index.html", "<html><body><h1>hi</h1></body></html>\n");
    t.write("static/style.css", "h1 { color: red; }\n");
    let rep = audit_json(&t.sub("static"), &["--top", "5"]);
    assert_eq!(rep["verdict"], "N/A");
    assert!(rep["unscored_top"].as_array().unwrap().iter().any(|e| e[0] == ".html"));
    assert!(rep["guidance"].as_array().unwrap().iter().any(|g| g.as_str().unwrap().contains("OUT OF SCOPE")));
    let (code, stdout, _) = audit(&t.sub("static"), &[]);
    assert_eq!(code, 0);
    assert!(stdout.contains("VERDICT: N/A") && stdout.contains("unscored (not analyzed)"));
}

// 8. Whole-repo focus: no baseline contrast.
#[test]
fn whole_repo_focus() {
    let t = TestDir::new();
    t.write("whole/pkg/a.py", "def a():\n    return 1\n");
    t.write("whole/pkg/b.py", "def b():\n    return 2\n");
    let rep = audit_json(&t.sub("whole"), &["--focus", "pkg/"]);
    assert!(rep["focus"]["assessment"].as_str().unwrap().to_lowercase().contains("no baseline contrast"));
}

// 14. Focus matches path-segment boundaries, not raw substrings.
#[test]
fn focus_boundaries() {
    let t = TestDir::new();
    t.write("seg/lib/foo.py", "def foo():\n    return 1\n");
    t.write("seg/lib/test_foo.py", "def test_foo():\n    assert True\n");
    let rep = audit_json(&t.sub("seg"), &["--focus", "foo.py"]);
    assert_eq!(rep["focus"]["files"], 1);
    assert!(rep["focus"]["top"][0]["path"].as_str().unwrap().ends_with("lib/foo.py"));
}

// 9. Test/prod split.
#[test]
fn test_prod_split() {
    let t = TestDir::new();
    t.write("lib/src/foo.py", "def add(a, b):\n    return a + b\n");
    let heavy: String = "import foo\n".to_string()
        + &(0..60)
            .map(|i| format!("def test_{i}():\n    assert foo.add({i}, 1) == {}\n", i + 1))
            .collect::<Vec<_>>()
            .join("");
    t.write("lib/tests/test_foo.py", &heavy);
    let rep = audit_json(&t.sub("lib"), &["--top", "5"]);
    assert_eq!(rep["test_files"], 1);
    assert_eq!(rep["prod_files"], 1);
    assert_eq!(rep["verdict"], "LOW");
    assert!(rep["hotspots"].as_array().unwrap().iter().all(|h| !h["path"].as_str().unwrap().contains("tests")));
    assert!(rep["test_hotspots"].as_array().unwrap().iter().any(|h| h["path"].as_str().unwrap().contains("test_foo")));
}

// 10. Vendored tooling never scanned.
#[test]
fn vendor_excluded() {
    let t = TestDir::new();
    t.write("vend/src/a.py", "def a():\n    return 1\n");
    t.write("vend/.yarn/releases/tool.cjs", &("function big() {\n".to_string() + &"  if (1) { x(); }\n".repeat(50) + "}\n"));
    t.write("vend/vendor/dep/dep.py", &("def dep():\n".to_string() + &"    if True:\n        pass\n".repeat(50)));
    let rep = audit_json(&t.sub("vend"), &["--top", "5"]);
    assert_eq!(rep["code_files"], 1);
    assert_eq!(rep["files_total"], 1);
}

// 11. Cycles: eager HIGH, lazy WATCH, TYPE_CHECKING silent.
#[test]
fn cycles() {
    let t = TestDir::new();
    t.write("cyc/a.py", "from b import B\n\ndef fa():\n    return B()\n");
    t.write("cyc/b.py", "from a import A\n\ndef fb():\n    return A()\n");
    let rc = audit_json(&t.sub("cyc"), &[]);
    assert!(rc["warnings"].as_array().unwrap().iter().any(|w| w["severity"] == "HIGH" && w["msg"].as_str().unwrap().contains("import cycle")));
    t.write(
        "tcyc/x.py",
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from y import Y\n\ndef fx():\n    return 1\n",
    );
    t.write(
        "tcyc/y.py",
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from x import X\n\ndef fy():\n    return 2\n",
    );
    let rt = audit_json(&t.sub("tcyc"), &[]);
    assert!(!rt["warnings"].as_array().unwrap().iter().any(|w| w["msg"].as_str().unwrap().contains("cycle")));
    // Qualified guards (`typing.TYPE_CHECKING`, `t.TYPE_CHECKING`) are erased too.
    t.write("qcyc/a.py", "from b import B\n\ndef fa():\n    return B()\n");
    t.write(
        "qcyc/b.py",
        "import typing as t\nif t.TYPE_CHECKING:\n    from a import A\n\ndef fb():\n    return 2\n",
    );
    let rq = audit_json(&t.sub("qcyc"), &[]);
    assert!(rq["cycles"].as_array().unwrap().is_empty(), "{}", rq["cycles"]);
    t.write("lcyc/p.py", "def fp():\n    from q import Q\n    return Q()\n");
    t.write("lcyc/q.py", "from p import P\n\ndef fq():\n    return P()\n");
    let rl = audit_json(&t.sub("lcyc"), &[]);
    let ws = rl["warnings"].as_array().unwrap();
    assert!(ws.iter().any(|w| w["severity"] == "WATCH" && w["msg"].as_str().unwrap().contains("lazy")));
    assert!(!ws.iter().any(|w| w["msg"].as_str().unwrap().contains("cycle") && w["severity"] == "HIGH"));
}

// 11d. Verification-only eager cycles stay visible but never HIGH:
// neutral-only SCC reads WATCH, mixed/prod SCC keeps HIGH.
#[test]
fn neutral_only_cycle_watch() {
    let t = TestDir::new();
    // Neutral-only: two test-dir files importing each other eagerly.
    t.write(
        "ncyc/test/a.ts",
        "import { b } from './b';\nexport function a(x: number) { return b(x); }\n",
    );
    t.write(
        "ncyc/test/b.ts",
        "import { a } from './a';\nexport function b(x: number) { return a(x); }\n",
    );
    let rep = audit_json(&t.sub("ncyc"), &[]);
    let ws = rep["warnings"].as_array().unwrap();
    assert!(
        ws.iter().any(|w| w["severity"] == "WATCH"
            && w["msg"].as_str().unwrap().contains("verification-only")),
        "{ws:?}"
    );
    assert!(
        !ws.iter().any(|w| w["severity"] == "HIGH"
            && w["msg"].as_str().unwrap().contains("cycle")),
        "{ws:?}"
    );
    // Mixed: one prod member restores HIGH.
    t.write(
        "mcyc/core.ts",
        "import { h } from './test/helper';\nexport function core(x: number) { return h(x); }\n",
    );
    t.write(
        "mcyc/test/helper.ts",
        "import { core } from '../core';\nexport function h(x: number) { return core(x); }\n",
    );
    let rep2 = audit_json(&t.sub("mcyc"), &[]);
    let ws2 = rep2["warnings"].as_array().unwrap();
    assert!(
        ws2.iter().any(|w| w["severity"] == "HIGH"
            && w["msg"].as_str().unwrap().contains("import cycle")),
        "{ws2:?}"
    );
}

// 11b. Type-only references draw no ripple claim: fan_in counts runtime
// edges only (same rule as cycles and focus blast radius).
#[test]
fn typeonly_no_fanin() {
    let t = TestDir::new();
    t.write("tco/hub_tc.py", "VALUE = 1\n");
    for i in 0..25 {
        t.write(
            &format!("tco/tc{i}.py"),
            "from __future__ import annotations\nfrom typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from hub_tc import VALUE\ndef f(v: \"VALUE\") -> \"VALUE\":\n    return v\n",
        );
    }
    t.write("tco/hub_mx.py", "ITEM = 1\n");
    t.write("tco/rt1.py", "from hub_mx import ITEM\nx = ITEM\n");
    t.write(
        "tco/rt2.py",
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from hub_mx import ITEM\ndef g(v: \"ITEM\") -> \"ITEM\":\n    return v\n",
    );
    t.write("tco/hub_ts.ts", "export const V = 1;\n");
    for i in 0..3 {
        t.write(
            &format!("tco/tt{i}.ts"),
            &format!("import type {{ V }} from './hub_ts';\nexport function f(v: V): V {{ return v; }}\n"),
        );
    }
    let rep = audit_json(&t.sub("tco"), &["--top", "40"]);
    let h = hotspots(&rep);
    // 25 annotation-only references: no fan_in, no coupling warning.
    assert_eq!(get(&h, "hub_tc.py")["fan_in"], 0);
    assert_eq!(get(&h, "hub_tc.py")["coupling"], 0);
    // Mixed hub: the runtime importer counts, the annotation one does not.
    assert_eq!(get(&h, "hub_mx.py")["fan_in"], 1);
    // Shared code path for TS `import type`.
    assert_eq!(get(&h, "hub_ts.ts")["fan_in"], 0);
    let ws = rep["warnings"].as_array().unwrap();
    assert!(!ws.iter().any(|w| w["where"].as_str().unwrap().ends_with("hub_tc.py")));
    assert!(!ws.iter().any(|w| w["where"].as_str().unwrap().ends_with("hub_ts.ts")));
}

// 11c. Type-only imports count toward neither fan-out split: upstream
// annotation-only references cannot break the importer at runtime.
#[test]
fn typeonly_no_fanout() {
    let t = TestDir::new();
    for i in 0..26 {
        t.write(&format!("tco/u{i}.py"), &format!("A{i} = 1\n"));
    }
    let mut body = String::from("from __future__ import annotations\nfrom typing import TYPE_CHECKING\n");
    for i in 1..26 {
        body.push_str(&format!("if TYPE_CHECKING:\n    from u{i} import A{i}\n"));
    }
    body.push_str("def f() -> None:\n    pass\n");
    t.write("tco/leaf_tc.py", &body);
    t.write(
        "tco/leaf_mx.py",
        "from u0 import A0\nfrom typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from u1 import A1\nx = A0\n",
    );
    let rep = audit_json(&t.sub("tco"), &["--top", "40"]);
    let h = hotspots(&rep);
    // 25 annotation-only references: no fan-out, no coupling warning.
    assert_eq!(get(&h, "leaf_tc.py")["fan_out_in"], 0);
    assert_eq!(get(&h, "leaf_tc.py")["coupling"], 0);
    // Mixed leaf: the runtime import counts, the annotation one does not.
    assert_eq!(get(&h, "leaf_mx.py")["fan_out_in"], 1);
    let ws = rep["warnings"].as_array().unwrap();
    assert!(!ws.iter().any(|w| w["where"].as_str().unwrap().ends_with("leaf_tc.py")));
}

// 12. Test-data/generated surfaces are verdict-neutral.
#[test]
fn testdata_generated_neutral() {
    let t = TestDir::new();
    t.write("gen/src/core.py", "def core():\n    return 1\n");
    t.write(
        "gen/testdata/big_fixture.py",
        &(0..80).map(|i| format!("def f{i}():\n    return {i}\n")).collect::<Vec<_>>().join(""),
    );
    t.write(
        "gen/src/api.generated.ts",
        &("export function g() {\n".to_string() + &"  if (1) { return 1; }\n".repeat(40) + "}\n"),
    );
    let rep = audit_json(&t.sub("gen"), &["--top", "5"]);
    assert_eq!(rep["test_files"], 2);
    assert_eq!(rep["prod_files"], 1);
    assert_eq!(rep["verdict"], "LOW");
}

// 13. Declaration files hold no behavior.
#[test]
fn decl_neutral() {
    let t = TestDir::new();
    t.write("decl/src/real.py", "def real():\n    return 1\n");
    t.write(
        "decl/src/big.d.ts",
        &(0..60).map(|i| format!("declare function f{i}(x: number): number;\n")).collect::<Vec<_>>().join(""),
    );
    t.write("decl/src/iface.pyi", "def stub(x: int) -> int: ...\n");
    let rep = audit_json(&t.sub("decl"), &["--top", "5"]);
    assert_eq!(rep["test_files"], 2);
    assert_eq!(rep["prod_files"], 1);
    assert_eq!(rep["verdict"], "LOW");
}

// 15. Determinism: identical reports across runs and worker counts.
#[test]
fn deterministic_across_runs_and_jobs() {
    let t = TestDir::new();
    t.write("seg/lib/foo.py", "def foo():\n    return 1\n");
    t.write("seg/lib/test_foo.py", "def test_foo():\n    assert True\n");
    let norm = |s: String| -> String {
        let v: Value = serde_json::from_str(&s).unwrap();
        let mut v = v;
        v.as_object_mut().unwrap().remove("elapsed_s");
        v.as_object_mut().unwrap().remove("phases");
        serde_json::to_string(&v).unwrap()
    };
    let (c1, o1, _) = audit(&t.sub("seg"), &["--json", "--top", "5"]);
    let (c2, o2, _) = audit(&t.sub("seg"), &["--json", "--top", "5"]);
    let (c3, o3, _) = audit(&t.sub("seg"), &["--json", "--top", "5", "--jobs", "1"]);
    assert_eq!((c1, c2, c3), (0, 0, 0));
    let n1 = norm(o1);
    assert_eq!(n1, norm(o2));
    assert_eq!(n1, norm(o3));
    assert_ne!(n1, "{}");
}

// 16. Clones: cross-folder warns; language mirrors silent.
#[test]
fn clones() {
    let t = TestDir::new();
    let block: String = (0..6).map(|i| format!("    v{i} = compute({i})\n    total += v{i}\n")).collect();
    t.write("dup/pkg/one.py", &format!("def run():\n    total = 0\n{block}    return total\n"));
    t.write("dup/other/two.py", &format!("def walk():\n    total = 0\n{block}    return total\n"));
    let mirror: String = (0..12).map(|i| format!("const v{i} = {i};\n")).collect();
    t.write("dup/other/port.ts", &mirror);
    t.write("dup/other/port.js", &mirror);
    let rep = audit_json(&t.sub("dup"), &["--top", "5"]);
    let clones = rep["clones"].as_array().unwrap();
    assert!(
        clones.iter().any(|c| {
            let pair = [c[0].as_str().unwrap().split('/').last().unwrap(), c[1].as_str().unwrap().split('/').last().unwrap()];
            pair.contains(&"one.py") && pair.contains(&"two.py")
        }),
        "{clones:?}"
    );
    assert!(rep["warnings"].as_array().unwrap().iter().any(|w| w["where"].as_str().unwrap().contains("one.py") && w["where"].as_str().unwrap().contains("two.py")));
    assert!(!clones.iter().any(|c| c[0].as_str().unwrap().contains("port") || c[1].as_str().unwrap().contains("port")));
    let (code, stdout, _) = audit(&t.sub("dup"), &[]);
    assert_eq!(code, 0);
    assert!(stdout.lines().any(|ln| ln.contains("dup=") && (ln.contains("one.py") || ln.contains("two.py"))));
}

// 17. Import hygiene: ?raw assets draw no edges; test-d is test kind.
#[test]
fn import_hygiene() {
    let t = TestDir::new();
    t.write(
        "hy/src/a.ts",
        "import txt from './tmpl.js?raw';\nimport { b } from './b';\nexport function a() { return b() + txt.length; }\n",
    );
    t.write("hy/src/tmpl.js", "import App from './App.vue';\nexport default {};\n");
    t.write("hy/src/b.ts", "export function b() { return 1; }\n");
    t.write("hy/dts-test/x.test-d.ts", "const v: number = 1;\nfunction g(): number { return v; }\n");
    let rep = audit_json(&t.sub("hy"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "src/b.ts")["fan_in"], 1);
    assert_eq!(get(&h, "src/tmpl.js")["fan_in"], 0);
    assert_eq!(rep["test_files"], 1);
}

// 18. Verdict coherence: HIGH floors at MODERATE.
#[test]
fn verdict_floor() {
    let t = TestDir::new();
    let mut body = "def process(items):\n    out = []\n".to_string();
    for i in 0..9 {
        body.push_str(&format!("    if items[{i}] > {i}:\n        out.append({i})\n    elif items[{i}] < 0:\n        out.append(0)\n"));
    }
    body.push_str("    return out\n");
    t.write("dil/core.py", &body);
    for i in 0..20 {
        t.write(&format!("dil/triv_{i}.py"), &format!("def t{i}():\n    return {i}\n"));
    }
    let rep = audit_json(&t.sub("dil"), &[]);
    assert!(["MODERATE", "HIGH", "SEVERE"].iter().any(|v| rep["verdict"] == *v));
    assert!(rep["warnings"].as_array().unwrap().iter().any(|w| w["severity"] == "HIGH"));
}

// 19. Most-specific import match wins.
#[test]
fn qualified_match_wins() {
    let t = TestDir::new();
    t.write("qual/lib/core/config.py", "VAL = 1\n");
    t.write("qual/ext/other/config.py", "VAL = 2\n");
    t.write("qual/app.py", "from lib.core.config import VAL\n\ndef f():\n    return VAL\n");
    let rep = audit_json(&t.sub("qual"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "lib/core/config.py")["fan_in"], serde_json::json!(1));
    assert_eq!(get(&h, "ext/other/config.py")["fan_in"], serde_json::json!(0));
}

// 20. Large tangles watch; small cycles alarm.
#[test]
fn tangle_watch() {
    let t = TestDir::new();
    for i in 0..10 {
        let nxt = (i + 1) % 10;
        t.write(&format!("tang/m{i}.py"), &format!("from m{nxt} import v{nxt}\nV{i} = {i}\n"));
    }
    let rep = audit_json(&t.sub("tang"), &[]);
    let tw: Vec<&Value> = rep["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["msg"].as_str().unwrap().contains("tangle") || w["msg"].as_str().unwrap().contains("cycle"))
        .collect();
    assert!(tw.iter().any(|w| w["msg"].as_str().unwrap().contains("tangle") && w["severity"] == "WATCH"));
    assert!(!tw.iter().any(|w| w["msg"].as_str().unwrap().contains("cycle") && w["severity"] == "HIGH"));
}

// 21. Performance guard: 150 files well under 20s.
#[test]
fn perf_bound() {
    let t = TestDir::new();
    t.write("perf/util.py", "def help(x):\n    return x\n");
    for i in 0..150 {
        t.write(
            &format!("perf/m{i}.py"),
            &format!("import os\nfrom util import help\n\ndef f{i}(x):\n    if x:\n        return help(x)\n    for i in range(3):\n        x += i\n    return x\n"),
        );
    }
    let start = std::time::Instant::now();
    let (code, _, stderr) = audit(&t.sub("perf"), &["--json", "--top", "3"]);
    let dt = start.elapsed();
    assert_eq!(code, 0, "{stderr}");
    assert!(dt.as_secs() < 20, "{dt:?}");
}

// 22. Phase timings present and roughly sum to elapsed.
#[test]
fn phases_add_up() {
    let t = TestDir::new();
    t.write("proj/a.py", "def a():\n    return 1\n");
    let rep = audit_json(&t.sub("proj"), &[]);
    let ph = rep["phases"].as_object().unwrap();
    let keys: std::collections::HashSet<&str> = ph.keys().map(|s| s.as_str()).collect();
    assert_eq!(keys, ["discovery_s", "read_parse_s", "relationships_s", "reporting_s"].into_iter().collect());
    let sum: f64 = ph.values().map(|v| v.as_f64().unwrap()).sum();
    assert!((sum - rep["elapsed_s"].as_f64().unwrap()).abs() < 0.5, "{ph:?}");
}

// 23. Subdirectory audit: absolute package imports still resolve.
#[test]
fn subdir_coupling() {
    let t = TestDir::new();
    t.write("mono/pkg/__init__.py", "");
    t.write("mono/pkg/sub/__init__.py", "");
    t.write("mono/pkg/sub/core.py", "from pkg.sub.util import help\n\ndef run():\n    return help()\n");
    t.write("mono/pkg/sub/util.py", "def help():\n    return 1\n");
    let rep = audit_json(&t.sub("mono/pkg/sub"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "util.py")["fan_in"], 1);
    assert_eq!(get(&h, "core.py")["fan_out_in"], 1);
}

// 24. JSON contract for agent consumers.
#[test]
fn json_contract() {
    let t = TestDir::new();
    t.write("proj/a.py", "def a():\n    return 1\n");
    let rep = audit_json(&t.sub("proj"), &["--top", "5"]);
    let obj = rep.as_object().unwrap();
    for k in ["cxcap_version", "root", "elapsed_s", "files_total", "code_files", "complexity",
              "prod_files", "prod_complexity", "test_files", "test_complexity", "hotspots",
              "folders", "warnings", "verdict", "guidance", "focus", "notes", "phases",
              "cycles", "clones"] {
        assert!(obj.contains_key(k), "missing {k}");
    }
    fn finite(v: &Value) {
        match v {
            Value::Number(n) => assert!(n.as_f64().unwrap().is_finite()),
            Value::Array(a) => a.iter().for_each(finite),
            Value::Object(o) => o.values().for_each(finite),
            _ => {}
        }
    }
    finite(&rep);
    let hs: Vec<f64> = rep["hotspots"].as_array().unwrap().iter().map(|h| h["hotspot"].as_f64().unwrap()).collect();
    assert!(hs.windows(2).all(|w| w[0] >= w[1]));
    assert!(rep["warnings"].as_array().unwrap().iter().all(|w| {
        let keys: std::collections::HashSet<&str> = w.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        keys == ["severity", "where", "msg"].into_iter().collect::<std::collections::HashSet<_>>()
    }));
}

// 25. Notebooks scored; broken JSON flagged.
#[test]
fn notebooks() {
    let t = TestDir::new();
    let nb = serde_json::json!({"cells": [
        {"cell_type": "code", "source": ["import os\n", "%matplotlib inline\n", "def f(x):\n", "    if x:\n", "        return 1\n", "    return 0\n", "f??\n"]},
        {"cell_type": "markdown", "source": ["# notes"]},
        {"cell_type": "code", "source": ["!ls\n", "y = f(1)\n"]}],
        "metadata": {}, "nbformat": 4, "nbformat_minor": 5});
    t.write("nb/analysis.ipynb", &serde_json::to_string(&nb).unwrap());
    t.write("nb/broken.ipynb", "{not json");
    let rep = audit_json(&t.sub("nb"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "analysis.ipynb")["funcs"], 1);
    assert_eq!(get(&h, "analysis.ipynb")["max_func_cx"], 2);
    assert!(rep["parse_errors"].as_array().unwrap().iter().any(|e| e == "broken.ipynb"));
    assert_eq!(get(&h, "broken.ipynb")["complexity"], 0);
}

// 26. Config surface counted and shown.
#[test]
fn config_surface() {
    let t = TestDir::new();
    t.write("cfg/a.py", "def a():\n    return 1\n");
    t.write("cfg/settings.yaml", "key: value\nlist:\n  - 1\n  - 2\n");
    t.write("cfg/pyproject.toml", "[tool]\nx = 1\n");
    let rep = audit_json(&t.sub("cfg"), &["--top", "5"]);
    assert_eq!(rep["config_files"], 2);
    assert_eq!(rep["config_loc"], 6);
    let (code, stdout, _) = audit(&t.sub("cfg"), &[]);
    assert_eq!(code, 0);
    assert!(stdout.contains("config: 2 files, 6 LOC"), "{}", stdout.lines().nth(1).unwrap_or(""));
}

// 27. God-folder mutual exclusion.
#[test]
fn god_exclusion() {
    let t = TestDir::new();
    for d in ["mirror_a", "mirror_b"] {
        for i in 0..6 {
            let body: String = (0..6).map(|j| format!("    if x == {j}:\n        x += 1\n")).collect();
            t.write(&format!("mir/{d}/m{i}.py"), &format!("def g(x):\n{body}    return x\n"));
        }
    }
    let rep = audit_json(&t.sub("mir"), &["--top", "5"]);
    assert!(!rep["warnings"].as_array().unwrap().iter().any(|w| w["msg"].as_str().unwrap().contains("god folder")));
    let big: String = (0..3)
        .map(|k| {
            let body: String = (0..16).map(|j| format!("    if x == {j}:\n        x += 1\n")).collect();
            format!("def g{k}(x):\n{body}    return x\n")
        })
        .collect();
    t.write("one/big/a.py", &big);
    for i in 0..9 {
        t.write(&format!("one/small/s{i}.py"), "def h():\n    return 1\n");
    }
    let rep2 = audit_json(&t.sub("one"), &["--top", "5"]);
    assert!(rep2["warnings"].as_array().unwrap().iter().any(|w| w["msg"].as_str().unwrap().contains("god folder")));
}

// 28. In-package vendoring excluded.
#[test]
fn underscore_vendor() {
    let t = TestDir::new();
    t.write("vp/src/app.py", "def app():\n    return 1\n");
    t.write("vp/src/_vendor/dep/big.py", &("def big():\n".to_string() + &"    x = 1\n".repeat(200)));
    let rep = audit_json(&t.sub("vp"), &["--top", "5"]);
    assert_eq!(rep["code_files"], 1);
    assert_eq!(rep["files_total"], 1);
}

// 29. Symlinks disclosed, never followed.
#[test]
#[cfg(unix)]
fn symlinks_disclosed() {
    let t = TestDir::new();
    t.write("sym/real/core.py", "def core():\n    return 1\n");
    std::os::unix::fs::symlink(t.sub("sym/real/core.py"), t.sub("sym/linked.py")).unwrap();
    std::os::unix::fs::symlink(t.sub("sym/real"), t.sub("sym/realdir")).unwrap();
    let rep = audit_json(&t.sub("sym"), &["--top", "5"]);
    assert_eq!(rep["skipped_symlinks"], 2);
    let (code, stdout, _) = audit(&t.sub("sym"), &[]);
    assert_eq!(code, 0);
    assert!(stdout.contains("symlinked path(s)"));
}

// 30. BOM parses; unreadable files disclosed.
#[test]
fn robust_reads() {
    let t = TestDir::new();
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend_from_slice(b"def bom():\n    return 1\n");
    t.write_bytes("rob/bom.py", &bom);
    t.write("rob/locked.py", "def locked():\n    return 2\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(t.sub("rob/locked.py"), fs::Permissions::from_mode(0)).unwrap();
    }
    // Root can still read chmod-0 files: skip unreadable assertions there.
    let still_readable = fs::read(t.sub("rob/locked.py")).is_ok();
    let rep = audit_json(&t.sub("rob"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "bom.py")["funcs"], 1);
    assert!(get(&h, "bom.py").get("parse_error").is_none());
    if !still_readable {
        assert_eq!(rep["skipped_unreadable"], 1);
        let (code, stdout, _) = audit(&t.sub("rob"), &[]);
        assert_eq!(code, 0);
        assert!(stdout.contains("unreadable file(s)"), "{stdout}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(t.sub("rob/locked.py"), fs::Permissions::from_mode(0o644)).unwrap();
    }
}

// 31. Warning list complete, no silent cap.
#[test]
fn warning_cap() {
    let t = TestDir::new();
    for i in 0..45 {
        let nest: String = (0..5).map(|d| format!("{}if x > {d}:\n", "    ".repeat(d + 1))).collect();
        t.write(&format!("cap/n{i}.py"), &format!("def f(x):\n{nest}      return x\n"));
    }
    // Each file nests 5 deep (func frame + 5 ifs): exactly one nesting
    // WATCH per file, nothing else — 45 warnings total, no silent cap.
    let rep = audit_json(&t.sub("cap"), &["--top", "5"]);
    assert_eq!(rep["warnings"].as_array().unwrap().len(), 45);
}

// 32. CLI surface: --version matches Cargo.toml; --max-bytes forces skip.
#[test]
fn cli_surface() {
    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let manifest = fs::read_to_string(common::pkg_root().join("Cargo.toml")).unwrap();
    let ver = manifest
        .lines()
        .find_map(|l| l.strip_prefix("version = "))
        .unwrap()
        .trim_matches('"')
        .to_string();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), ver);
    assert!(!ver.is_empty());
    let t = TestDir::new();
    t.write("proj/a.py", "def a():\n    return 1\n");
    t.write("proj/b.ts", "export const x = 1;\n");
    let rep = audit_json(&t.sub("proj"), &["--max-bytes", "10"]);
    assert_eq!(rep["code_files"], 0);
    assert_eq!(rep["skipped_oversize"], 2);
    assert_eq!(rep["verdict"], "N/A");
}

// 33. Extensionless Python scored; shell ignored.
#[test]
fn shebang() {
    let t = TestDir::new();
    t.write("sh/runme", "#!/usr/bin/env python\ndef go():\n    return 1\n");
    t.write("sh/build.sh", "#!/bin/sh\necho hi\n");
    let rep = audit_json(&t.sub("sh"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "runme")["funcs"], 1);
    assert!(!h.contains_key("build.sh"));
    assert!(rep["unscored_top"].as_array().unwrap().iter().any(|e| e[0] == ".sh"));
}

// 34. export...from counts as a dependency edge.
#[test]
fn reexport_edge() {
    let t = TestDir::new();
    t.write("bar/y.ts", "export function y() { return 1; }\n");
    t.write(
        "bar/index.ts",
        "export { y } from './y';\nimport { y as z } from './y';\nexport const w = z();\n",
    );
    let rep = audit_json(&t.sub("bar"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "y.ts")["fan_in"], 2);
}

// 35. Deep trees keep structure.
#[test]
fn deep_folders() {
    let t = TestDir::new();
    for (d, _) in [("a/b/c", 3), ("a/b/d", 1)] {
        for i in 0..2 {
            t.write(&format!("deep/{d}/m{i}.py"), "def g(x):\n    if x:\n        return 1\n    return 0\n");
        }
    }
    let rep = audit_json(&t.sub("deep"), &["--top", "5"]);
    let dirs: Vec<&str> = rep["folders"].as_array().unwrap().iter().map(|f| f["dir"].as_str().unwrap()).collect();
    assert!(dirs.iter().any(|d| d.ends_with("a/b/c")) && dirs.iter().any(|d| d.ends_with("a/b/d")), "{dirs:?}");
}

// 36. Modern Python syntax scored; t-strings fall back gracefully.
#[test]
fn modern_python() {
    let t = TestDir::new();
    t.write(
        "modern/pep695.py",
        "type Alias = dict[str, int]\n\ndef f[T](x: T) -> T:\n    try:\n        return x\n    except* ValueError:\n        return x\n",
    );
    t.write(
        "modern/tstr.py",
        "def g(name):\n    t = t\"hi {name}\"\n    if t:\n        return 1\n    return 0\n",
    );
    let rep = audit_json(&t.sub("modern"), &["--top", "5"]);
    let h = hotspots(&rep);
    // except* + try/except counted; type statement + type param add nothing.
    assert_eq!(get(&h, "pep695.py")["funcs"], 1);
    assert!(get(&h, "pep695.py").get("parse_error").is_none());
    // t-strings parse without error nodes here, scoring like 3.14 does.
    assert_eq!(get(&h, "tstr.py")["funcs"], 1);
    assert_eq!(get(&h, "tstr.py")["max_func_cx"], 2);
    assert!(get(&h, "tstr.py").get("parse_error").is_none());
}

// 37. Modern TS idioms don't break the audit.
#[test]
fn modern_ts() {
    let t = TestDir::new();
    t.write(
        "tsmod/c.ts",
        "function dec(target: any) { return target; }\n@dec\nexport class C {\n  m(x: number) {\n    using r = acquire();\n    const y = x satisfies number;\n    if (y > 1) { return 1; }\n    return 0;\n  }\n}\nimport data from './data.json' with { type: 'json' };\n",
    );
    let (code, _, stderr) = audit(&t.sub("tsmod"), &["--json", "--top", "5"]);
    assert_eq!(code, 0, "{stderr}");
    let rep = audit_json(&t.sub("tsmod"), &["--top", "5"]);
    assert_eq!(rep["code_files"], 1);
    assert!(rep["complexity"].as_u64().unwrap() >= 3);
}

// 38. Hostile files never kill the audit.
#[test]
fn hostile_files() {
    let t = TestDir::new();
    t.write("hostile/deep.py", &("x = ".to_string() + &vec!["1"; 20000].join("+") + "\n"));
    t.write_bytes("hostile/nul.py", b"x = 1\n\x00\n");
    let rep = audit_json(&t.sub("hostile"), &["--top", "5"]);
    let mut errs: Vec<&str> = rep["parse_errors"].as_array().unwrap().iter().map(|e| e.as_str().unwrap()).collect();
    errs.sort();
    assert_eq!(errs, ["deep.py", "nul.py"]);
}

// 39. Workspace package specifiers are internal edges.
#[test]
fn workspace_edges() {
    let t = TestDir::new();
    t.write("ws/packages/shared/package.json", r#"{"name": "@corp/shared", "main": "lib/main.js"}"#);
    t.write("ws/packages/shared/lib/main.ts", "export function s(x: number) { return x; }\n");
    t.write("ws/packages/app/package.json", r#"{"name": "@corp/app"}"#);
    t.write(
        "ws/packages/app/index.ts",
        "import { s } from '@corp/shared';\nimport r from '@corp/shared/lib/main';\nexport function a(x: number) { if (x) { return s(x) + r(x); } return 0; }\n",
    );
    t.write("ws/package.json", r#"{"name": "root-mono"}"#);
    t.write("ws/core/package.json", r#"{"name": "wcore"}"#);
    t.write(
        "ws/core/index.ts",
        "import { s } from '@corp/shared';\nexport function c(x: number) { return s(x); }\n",
    );
    t.write(
        "ws/core/ext.ts",
        "import React from 'react';\nimport path from 'path';\nexport const e = 1;\n",
    );
    let rep = audit_json(&t.sub("ws"), &["--top", "8"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "packages/shared/lib/main.ts")["fan_in"], 3);
    assert_eq!(get(&h, "packages/app/index.ts")["fan_out_in"], 2);
    assert_eq!(get(&h, "packages/app/index.ts")["fan_out_ex"], 0);
    assert!(h.values().filter(|v| v["path"].as_str().unwrap().ends_with("ext.ts")).all(|v| v["fan_out_ex"].as_u64().unwrap() >= 2));
}

// 40. Cross-package workspace import cycle detected.
#[test]
fn workspace_cycle() {
    let t = TestDir::new();
    t.write("wsc/pkgA/package.json", r#"{"name": "pkg-a"}"#);
    t.write("wsc/pkgA/index.ts", "import { b } from 'pkg-b';\nexport function a(x: number) { return b(x); }\n");
    t.write("wsc/pkgB/package.json", r#"{"name": "pkg-b"}"#);
    t.write("wsc/pkgB/index.ts", "import { a } from 'pkg-a';\nexport function b(x: number) { return a(x); }\n");
    let rep = audit_json(&t.sub("wsc"), &["--top", "5"]);
    assert!(rep["warnings"].as_array().unwrap().iter().any(|w| w["msg"].as_str().unwrap().contains("import cycle")));
}

// 41. Directory-index relative imports resolve.
#[test]
fn dir_index() {
    let t = TestDir::new();
    t.write("dirx/src/widgets/index.ts", "export function w(x: number) { return x; }\n");
    t.write(
        "dirx/src/app.ts",
        "import { w } from './widgets';\nexport function a(x: number) { if (x) { return w(x); } return 0; }\n",
    );
    let rep = audit_json(&t.sub("dirx"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "src/widgets/index.ts")["fan_in"], 1);
    assert_eq!(get(&h, "src/app.ts")["fan_out_in"], 1);
}

// 42. Demo/tutorial surfaces are verdict-neutral.
#[test]
fn demo_neutral() {
    let t = TestDir::new();
    t.write(
        "demo/lib/core.py",
        "def normalize(items):\n    out = []\n    for it in items:\n        if it is None:\n            continue\n        if isinstance(it, str):\n            if it.strip():\n                out.append(it.strip().lower())\n    return out\n",
    );
    let demo = "from lib.core import normalize\n\ndef run(rows, strict):\n    out = []\n    for row in rows:\n        if row is None:\n            if strict:\n                raise ValueError('null')\n            else:\n                continue\n        elif isinstance(row, str):\n            text = row.strip()\n            if not text:\n                continue\n            elif text.startswith('#'):\n                continue\n            elif len(text) > 80:\n                out.append(text[:80])\n            else:\n                out.append(text.lower())\n        else:\n            raise TypeError(row)\n    return normalize(out)\n";
    for i in 0..3 {
        t.write(&format!("demo/examples/demo_{i}.py"), &demo.replace("def run(", &format!("def run_{i}(")));
    }
    let rep = audit_json(&t.sub("demo"), &["--top", "5"]);
    assert_eq!(rep["prod_files"], 1);
    assert_eq!(rep["test_files"], 3);
    assert_eq!(rep["hotspots"].as_array().unwrap().len(), 1);
    assert_eq!(rep["hotspots"][0]["path"], "lib/core.py");
    assert!(rep["test_hotspots"].as_array().unwrap().iter().all(|h| h["path"].as_str().unwrap().contains("examples")));
    assert!(rep["warnings"].as_array().unwrap().iter().all(|w| !w["where"].as_str().unwrap().contains("examples")));
    t.write("tute/lib/core.py", "def core():\n    return 1\n");
    t.write("tute/tutorial/step1.py", "def step():\n    return 1\n");
    let rep2 = audit_json(&t.sub("tute"), &["--top", "5"]);
    assert_eq!(rep2["prod_files"], 1);
    assert_eq!(rep2["test_files"], 1);
}

// 43. E2E suites are verification surface.
#[test]
fn e2e_neutral() {
    let t = TestDir::new();
    t.write("e2e/lib/core.ts", "export function core(x: number) { return x; }\n");
    t.write(
        "e2e/api/x.controller.e2e-spec.ts",
        "import { core } from '../lib/core';\nexport function check(x: number) { if (x) { return core(x); } return 0; }\n",
    );
    t.write(
        "e2e/web/y.e2e.ts",
        "import { core } from '../lib/core';\nexport function drive(x: number) { if (x) { return core(x); } return 0; }\n",
    );
    t.write(
        "e2e/playwright/z.ts",
        "import { core } from '../lib/core';\nexport function flow(x: number) { return core(x); }\n",
    );
    t.write("e2e/lib/fee2e_helper.ts", "export function help(x: number) { return x; }\n");
    t.write("e2e/cypress/w.ts", "export function w(x: number) { return x; }\n");
    let rep = audit_json(&t.sub("e2e"), &["--top", "8"]);
    assert_eq!(rep["prod_files"], 3);
    assert_eq!(rep["test_files"], 3);
    assert!(rep["hotspots"].as_array().unwrap().iter().all(|h| {
        let p = h["path"].as_str().unwrap();
        !p.contains("e2e-spec") && !p.contains("playwright") && !p.ends_with("y.e2e.ts")
    }));
    assert!(rep["hotspots"].as_array().unwrap().iter().any(|h| h["path"].as_str().unwrap().ends_with("cypress/w.ts")));
    assert!(rep["hotspots"].as_array().unwrap().iter().any(|h| h["path"].as_str().unwrap().ends_with("fee2e_helper.ts")));
}

// 43b. DT-style `-tests.ts` suffix is verification surface.
#[test]
fn dash_tests_neutral() {
    let t = TestDir::new();
    t.write("dt/lib/core.ts", "export function core(x: number) { return x; }\n");
    t.write(
        "dt/lib/core-tests.ts",
        "import { core } from './core';\nexport function check(x: number) { if (x) { return core(x); } return 0; }\n",
    );
    t.write(
        "dt/lib/widget-tests.tsx",
        "import { core } from './core';\nexport function w(x: number) { return core(x); }\n",
    );
    // Negatives: no hyphen (`contest`), `-tests` not followed by a dot.
    t.write("dt/lib/contest.ts", "export function c(x: number) { return x; }\n");
    t.write("dt/lib/my-testsuite.ts", "export function s(x: number) { return x; }\n");
    t.write("dt/lib/edge.test.mjs", "export function e(x) { return x; }\n");
    t.write("dt/lib/edge.test.cjs", "exports.c = (x) => x;\n");
    let rep = audit_json(&t.sub("dt"), &["--top", "8"]);
    assert_eq!(rep["prod_files"], 3);
    assert_eq!(rep["test_files"], 4);
    assert!(rep["hotspots"].as_array().unwrap().iter().all(|h| {
        let p = h["path"].as_str().unwrap();
        !p.contains("-tests.")
    }));
    assert!(rep["hotspots"].as_array().unwrap().iter().any(|h| h["path"].as_str().unwrap().ends_with("contest.ts")));
    assert!(rep["hotspots"].as_array().unwrap().iter().any(|h| h["path"].as_str().unwrap().ends_with("my-testsuite.ts")));
}

// 44. Generated code neutral; generator tools stay prod.
#[test]
fn generated_neutral() {
    let t = TestDir::new();
    let engine = "def handle(r):\n".to_string()
        + &(0..12).map(|i| format!("    if r == {i}:\n        return {i}\n")).collect::<Vec<_>>().join("")
        + "    return -1\n";
    t.write("gen2/src/engine.py", &engine);
    t.write(
        "gen2/src/engine_mirror.py",
        &format!("\"\"\"THIS FILE IS AUTO-GENERATED - DO NOT EDIT. Source: engine.py\"\"\"\n{engine}"),
    );
    t.write(
        "gen2/src/generated/icons.py",
        &("ICONS = {\n".to_string() + &(0..300).map(|i| format!("    \"{i}\": {i},\n")).collect::<Vec<_>>().join("") + "}\n"),
    );
    t.write(
        "gen2/src/emitter.py",
        "const header = `// This file was generated by custom gen, do not edit`;\nexport function emit() { return header; }\n",
    );
    t.write(
        "gen2/src/flags.py",
        "# Render function generated by the compiler; keep in sync manually.\nFLAGS = {}\n",
    );
    t.write(
        "gen2/src/ast_generated.rs",
        "// This is a generated file. Don't modify it by hand!\nfn big() -> i32 {\n    1\n}\n",
    );
    t.write(
        "gen2/src/queries.ts",
        "// generated with @7nohe/openapi-react-query-codegen@1.6.2\nexport function useA(x: number) {\n  if (x == 0) return 0;\n  if (x == 1) return 1;\n  if (x == 2) return 2;\n  if (x == 3) return 3;\n  if (x == 4) return 4;\n  if (x == 5) return 5;\n  if (x == 6) return 6;\n  if (x == 7) return 7;\n  if (x == 8) return 8;\n  if (x == 9) return 9;\n  if (x == 10) return 10;\n  if (x == 11) return 11;\n  return -1;\n}\n",
    );
    let rep = audit_json(&t.sub("gen2"), &["--top", "8"]);
    assert_eq!(rep["prod_files"], 3);
    assert_eq!(rep["test_files"], 4);    assert!(rep["hotspots"].as_array().unwrap().iter().all(|h| {
        let p = h["path"].as_str().unwrap();
        !p.contains("mirror") && !p.contains("generated")
    }));
    assert!(rep["hotspots"].as_array().unwrap().iter().any(|h| h["path"].as_str().unwrap().ends_with("emitter.py")));
    assert!(rep["hotspots"].as_array().unwrap().iter().any(|h| h["path"].as_str().unwrap().ends_with("flags.py")));
    assert!(rep["hotspots"].as_array().unwrap().iter().all(|h| !h["path"].as_str().unwrap().ends_with("queries.ts")));
    assert!(!rep["clones"].as_array().unwrap().iter().any(|c| c[0].as_str().unwrap().contains("engine_mirror") || c[1].as_str().unwrap().contains("engine_mirror")));
    assert!(!rep["clones"].as_array().unwrap().iter().any(|c| c[0].as_str().unwrap().contains("queries.ts") || c[1].as_str().unwrap().contains("queries.ts")));
}

// 44b. Minified/bundled output is verdict-neutral (mean line length).
#[test]
fn minified_neutral() {
    let t = TestDir::new();
    let core = "export function core(x: number) {\n".to_string()
        + &(0..30).map(|i| format!("  if (x === {i}) {{ return {i}; }}\n")).collect::<Vec<_>>().join("")
        + "  return -1;\n}\n";
    t.write("min/lib/core.ts", &core);
    // Bundle sludge: more branches than core, compacted onto long lines.
    let funcs: Vec<String> = (0..60).map(|i| format!("function p{i}(a){{if(a){{return a;}}return {i};}}")).collect();
    let bundle: String = funcs.chunks(6).map(|c| c.join(" ")).collect::<Vec<_>>().join("\n") + "\n";
    assert!(bundle.len() as u64 > 200 * bundle.lines().count() as u64);
    t.write("min/lib/vendor_bundle.js", &bundle);
    // Negative: one long line among short ones stays production.
    t.write(
        "min/lib/longline.ts",
        &("export const x = 1;\n".repeat(20) + &("export const url = \"https://example.com/".to_string() + &"a".repeat(400) + "\";\n")),
    );
    let rep = audit_json(&t.sub("min"), &["--top", "8"]);
    assert_eq!(rep["prod_files"], 2);
    assert_eq!(rep["test_files"], 1);
    let hs = rep["hotspots"].as_array().unwrap();
    assert!(hs.iter().all(|h| !h["path"].as_str().unwrap().contains("vendor_bundle")));
    assert!(hs.iter().any(|h| h["path"].as_str().unwrap().ends_with("core.ts")));
    assert!(hs.iter().any(|h| h["path"].as_str().unwrap().ends_with("longline.ts")));
    assert!(hs[0]["path"].as_str().unwrap().ends_with("core.ts"));
}

// 45. JS branch parity: ternary + ?. count; ?: params excluded.
#[test]
fn js_branch_parity() {
    let t = TestDir::new();
    t.write("br/t.ts", "export function f(x: number) {\n  return x ? 1 : 0;\n}\n");
    t.write("br/o.ts", "export function g(o: any) {\n  return o?.x ?? 'd';\n}\n");
    t.write(
        "br/n.ts",
        "export interface O {\n  opt?: number;\n}\nexport function h(a?: string) {\n  return a ?? 'x';\n}\n",
    );
    t.write("br/p.py", "def f(x):\n    return 1 if x else 0\n");
    t.write("br/q.js", "export function f(x) {\n  return x ? 1 : 0;\n}\n");
    let rep = audit_json(&t.sub("br"), &["--top", "8"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "t.ts")["complexity"], 3);
    assert_eq!(get(&h, "o.ts")["complexity"], 4);
    assert_eq!(get(&h, "n.ts")["complexity"], 3);
    assert_eq!(get(&h, "p.py")["complexity"], get(&h, "q.js")["complexity"]);
    assert_eq!(get(&h, "p.py")["complexity"], 3);
}

// 46. JS nesting parity.
#[test]
fn js_nesting_parity() {
    let t = TestDir::new();
    t.write(
        "nst/deep.ts",
        "export function f(x) {\n  if (x) {\n    for (const i of y) {\n      while (z) {\n        try {\n        } catch (e) {\n        }\n      }\n    }\n  }\n  return 1;\n}\n",
    );
    t.write(
        "nst/deep.py",
        "def f(x):\n    if x:\n        for i in y:\n            while z:\n                try:\n                    pass\n                except E:\n                    pass\n    return 1\n",
    );
    t.write(
        "nst/data.ts",
        "export const theme = {\n  colors: {\n    primary: {\n      light: '#fff',\n    },\n  },\n};\n",
    );
    t.write("nst/cls.ts", "class C {\n  m() {\n    if (x) {}\n  }\n}\n");
    let rep = audit_json(&t.sub("nst"), &["--top", "8"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "deep.ts")["max_nesting"], 5);
    assert_eq!(get(&h, "deep.py")["max_nesting"], 5);
    assert_eq!(get(&h, "data.ts")["max_nesting"], 0);
    assert_eq!(get(&h, "cls.ts")["max_nesting"], 3);
}

// 46b. Decision chains read as one level, not N deep.
#[test]
fn elif_chain_depth() {
    let t = TestDir::new();
    let mut py = String::from("def f(x):\n");
    for i in 0..10 {
        py.push_str(&format!(
            "    {} x == {}:\n        return {}\n",
            if i == 0 { "if" } else { "elif" },
            i,
            i
        ));
    }
    py.push_str("    else:\n        return -1\n");
    t.write("ch/chain.py", &py);
    let mut rs = String::from("fn f(x: i32) -> i32 {\n");
    for i in 0..10 {
        rs.push_str(&format!(
            "    {} x == {} {{\n        return {};\n",
            if i == 0 { "if" } else { "} else if" },
            i,
            i
        ));
    }
    rs.push_str("    } else {\n        return -1;\n    }\n}\n");
    t.write("ch/chain.rs", &rs);
    let mut ts = String::from("export function f(x: number) {\n");
    for i in 0..10 {
        ts.push_str(&format!(
            "  {} (x === {}) {{\n    return {};\n",
            if i == 0 { "if" } else { "} else if" },
            i,
            i
        ));
    }
    ts.push_str("  } else {\n    return -1;\n  }\n}\n");
    t.write("ch/chain.ts", &ts);
    // True depth still accumulates through a chain link.
    t.write(
        "ch/thru.py",
        "def g(y):\n    for i in y:\n        if i == 0:\n            tick()\n        elif i == 1:\n            if i:\n                tock()\n        else:\n            idle()\n",
    );
    // Match alternatives already share one level (guard against regressions).
    let mut mt = String::from("def h(x):\n    match x:\n");
    for i in 0..10 {
        mt.push_str(&format!("        case {}:\n            return {}\n", i, i));
    }
    mt.push_str("        case _:\n            return -1\n");
    t.write("ch/match.py", &mt);
    let rep = audit_json(&t.sub("ch"), &["--top", "8"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "chain.py")["max_nesting"], 2);
    assert_eq!(get(&h, "chain.rs")["max_nesting"], 2);
    // Terminal block `else` keeps its control scope in JS (documented:
    // `else` nests without branching), so the TS twin reads one deeper.
    assert_eq!(get(&h, "chain.ts")["max_nesting"], 3);
    // Branch counting untouched: identical shapes, identical complexity.
    assert_eq!(get(&h, "chain.py")["complexity"], 12);
    assert_eq!(get(&h, "chain.rs")["complexity"], 12);
    assert_eq!(get(&h, "chain.ts")["complexity"], 12);
    assert_eq!(get(&h, "thru.py")["max_nesting"], 4);
    assert_eq!(get(&h, "match.py")["max_nesting"], 2);
}

// 47. Output plurals.
#[test]
fn plurals() {
    let t = TestDir::new();
    t.write("pl/only.py", "def f():\n    return 1\n");
    let (code, stdout, _) = audit(&t.sub("pl"), &[]);
    assert_eq!(code, 0);
    assert!(stdout.contains("scanned 1 file in") && stdout.contains("code: 1 file,"));
    assert!(!stdout.contains("1 files"));
    t.write(
        "pl2/deep.py",
        "def f(x):\n    if x:\n        for i in y:\n            while z:\n                try:\n                    pass\n                except E:\n                    pass\n    return 1\n",
    );
    t.write("pl2/triv.py", "def t():\n    return 0\n");
    let rep = audit_json(&t.sub("pl2"), &["--top", "5"]);
    assert_eq!(rep["verdict"], "LOW");
    assert!(rep["verdict_why"].as_str().unwrap().contains("1 warning"));
    assert!(!rep["verdict_why"].as_str().unwrap().contains("1 warnings"));
    let repf = audit_json(&t.sub("pl2"), &["--focus", "deep.py"]);
    assert!(repf["focus"]["assessment"].as_str().unwrap().contains("across 1 file "));
    assert!(!repf["focus"]["assessment"].as_str().unwrap().contains("1 files"));
}

// 48. Focus surfaces: test-only focus; trailing slashes; ./ prefixes.
#[test]
fn focus_surfaces() {
    let t = TestDir::new();
    t.write("fcs/lib/core.py", "def core():\n    return 1\n");
    t.write(
        "fcs/tests/test_a.py",
        &(0..8).map(|i| format!("def test_{i}():\n    assert {i} < 100\n")).collect::<Vec<_>>().join(""),
    );
    let rep = audit_json(&t.sub("fcs"), &["--focus", "tests/"]);
    assert!(rep["focus"]["assessment"].as_str().unwrap().contains("verification surface"));
    assert!(!rep["focus"]["assessment"].as_str().unwrap().contains("expensive"));
    let rep2 = audit_json(&t.sub("fcs"), &["--focus", "core.py/"]);
    assert_eq!(rep2["focus"]["files"], 1);
    assert!(rep2["focus"]["top"][0]["path"].as_str().unwrap().ends_with("lib/core.py"));
    let rep3 = audit_json(&t.sub("fcs"), &["--focus", "./tests"]);
    assert_eq!(rep3["focus"]["files"], rep["focus"]["files"]);
}

// 49. Svelte SFC.
#[test]
fn svelte_sfc() {
    let t = TestDir::new();
    t.write(
        "sv/Card.svelte",
        "<script lang=\"ts\">\n  import { fmt } from './fmt';\n  export let name: string;\n  function inc() {\n    if (count > 10) { return; }\n    count += 1;\n  }\n</script>\n<div>{#if count > 5}<p>many for while if prose?</p>{/if}\n<button on:click={() => inc()}>Hi {name}</button></div>\n<style>.card { color: red; }</style>\n",
    );
    t.write("sv/fmt.ts", "export function fmt(x: string) { return x; }\n");
    let rep = audit_json(&t.sub("sv"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "Card.svelte")["complexity"], 3);
    assert_eq!(get(&h, "Card.svelte")["funcs"], 1);
    assert_eq!(get(&h, "fmt.ts")["fan_in"], 1);
}

// 50. Focus blast radius.
#[test]
fn blast_radius() {
    let t = TestDir::new();
    t.write("bl/lib/core.py", "def run(x):\n    if x:\n        return 1\n    return 0\n");
    t.write("bl/lib/util.py", "def help():\n    return 1\n");
    t.write(
        "bl/app/web.py",
        "from lib.core import run\n\ndef serve(q):\n    if q:\n        return run(q)\n    return 0\n",
    );
    t.write(
        "bl/app/cli.py",
        "from lib.core import run\nfrom lib.util import help\n\ndef main():\n    return run(help())\n",
    );
    t.write(
        "bl/tests/test_core.py",
        "from lib.core import run\n\ndef test_r():\n    assert run(1) == 1\n",
    );
    t.write("bl/cyc/a.py", "from cyc.b import g\n\ndef f():\n    return g()\n");
    t.write("bl/cyc/b.py", "from cyc.a import f\n\ndef g():\n    return f()\n");
    let rep = audit_json(&t.sub("bl"), &["--focus", "lib/"]);
    let f = &rep["focus"];
    assert_eq!(f["n_dependents"], 3);
    let paths: std::collections::HashSet<&str> = f["dependents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        ["app/web.py", "app/cli.py", "tests/test_core.py"].into_iter().collect::<std::collections::HashSet<_>>()
    );
    assert!(f["dependents"].as_array().unwrap().iter().any(|d| d["kind"] == "test"));
    assert!(f["hotspots_inside"].as_array().unwrap().iter().any(|p| p == "lib/core.py"));
    assert!(f["assessment"].as_str().unwrap().contains("repo hotspot"));
    assert!(f["assessment"].as_str().unwrap().contains("Imported by 3 outside files"));
    let rep2 = audit_json(&t.sub("bl"), &["--focus", "cyc/a.py"]);
    let f2 = &rep2["focus"];
    assert_eq!(
        f2["cycles_through"],
        serde_json::json!([{"members": ["cyc/a.py", "cyc/b.py"], "eager": true}])
    );
    assert!(f2["assessment"].as_str().unwrap().contains("2-module cycle"));
    assert!(f2["assessment"].as_str().unwrap().contains("import cycle"));
    assert!(f2["assessment"].as_str().unwrap().contains("Imported by 1 outside file"));
    let (code, stdout, _) = audit(&t.sub("bl"), &["--focus", "lib/"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("DEPENDENTS (3 outside)") && stdout.contains("app/web.py"));
}

// 51. JS per-function complexity parity.
#[test]
fn max_func_cx_parity() {
    let t = TestDir::new();
    t.write(
        "pf/dense.ts",
        &("export function process(x: number) {\n".to_string()
            + &(0..16).map(|i| format!("  if (x === {i}) {{ return {i}; }}\n")).collect::<Vec<_>>().join("")
            + "  return 0;\n}\n"),
    );
    t.write(
        "pf/dense.py",
        &("def process(x):\n".to_string()
            + &(0..16).map(|i| format!("    if x == {i}:\n        return {i}\n")).collect::<Vec<_>>().join("")
            + "    return 0\n"),
    );
    t.write(
        "pf/flat.ts",
        "export interface Big {\n  a: string;\n  nested: {\n    deep: {\n      x: number;\n    };\n  };\n}\nexport const theme = {\n  colors: {\n    primary: '#fff',\n  },\n};\n",
    );
    let rep = audit_json(&t.sub("pf"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "dense.ts")["max_func_cx"], 17);
    assert_eq!(get(&h, "dense.py")["max_func_cx"], 17);
    assert!(rep["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["severity"] == "HIGH" && w["where"] == "dense.ts" && w["msg"].as_str().unwrap().contains("function complexity 17")));
    assert_eq!(get(&h, "flat.ts")["max_func_cx"], 0);
}

// 52. CPython-verbatim error messages (locked catalog, not buckets).
#[test]
fn error_messages_verbatim() {
    let t = TestDir::new();
    t.write("err/zero.py", "x = 0755\n");
    t.write("err/badnum.py", "x = 1foo\n");
    t.write("err/bindigit.py", "x = 0b2\n");
    t.write_bytes("err/nul.py", b"x = 1\n\x00\n");
    t.write("err/nest.py", &("x = ".to_string() + &vec!["("; 210].join("\n") + "1" + &vec![")"; 210].join("")));
    let rep = audit_json(&t.sub("err"), &["--top", "5"]);
    let h = hotspots(&rep);
    let msg = |p: &str| get(&h, p)["parse_error"].as_str().unwrap().to_string();
    assert!(msg("zero.py").starts_with("leading zeros in decimal integer literals"), "{}", msg("zero.py"));
    assert_eq!(msg("badnum.py"), "invalid decimal literal @ line 1");
    assert_eq!(msg("bindigit.py"), "invalid digit '2' in binary literal @ line 1");
    assert_eq!(msg("nul.py"), "source code string cannot contain null bytes @ line None");
    assert_eq!(msg("nest.py"), "too many nested parentheses @ line 201");
}

// 53. PEP 696 defaults scored in place (branchy defaults count once).
#[test]
fn pep696_defaults() {
    let t = TestDir::new();
    t.write(
        "gen/class.py",
        "class C[T: Bound = Default](Base):\n    pass\n",
    );
    t.write(
        "gen/branchy.py",
        "def f[T = (1 if x else 2)](y):\n    return y\n",
    );
    let rep = audit_json(&t.sub("gen"), &["--top", "5"]);
    let h = hotspots(&rep);
    assert!(get(&h, "class.py").get("parse_error").is_none());
    assert_eq!(get(&h, "class.py")["classes"], 1);
    // default IfExp branch counts into f (1 base + 1 branch = 2).
    assert_eq!(get(&h, "branchy.py")["max_func_cx"], 2);
}

// 54. Clone hashing covers analyzed code, not wrapper text: notebook JSON
// boilerplate and SFC markup must not manufacture shared windows.
fn notebook_doc(blocks: &[&str]) -> String {
    let cells: Vec<Value> = blocks
        .iter()
        .map(|b| serde_json::json!({"cell_type": "code", "source": [*b]}))
        .collect();
    serde_json::json!({"cells": cells, "metadata": {}, "nbformat": 4, "nbformat_minor": 5}).to_string()
}

const SHARED_BLOCK: &str = "def shared(a, b):\n    total = a + b\n    if total > 10:\n        total = total * 2\n    for i in range(3):\n        total += i\n    while total < 0:\n        total += 1\n    return total\n";

#[test]
fn notebook_clones_hash_code() {
    let t = TestDir::new();
    t.write("nb/one.ipynb", &notebook_doc(&[SHARED_BLOCK, "def unique_one():\n    return 1\n"]));
    t.write("nb/two.ipynb", &notebook_doc(&[SHARED_BLOCK, "def unique_two():\n    return 2\n"]));
    // Same boilerplate, disjoint tiny code: no shared windows possible.
    t.write("nb/aaa.ipynb", &notebook_doc(&["x = 1\n"]));
    t.write("nb/bbb.ipynb", &notebook_doc(&["y = 2\n"]));
    let rep = audit_json(&t.sub("nb"), &["--top", "8"]);
    let clones = rep["clones"].as_array().unwrap();
    assert!(
        clones.iter().any(|c| {
            let pair = [c[0].as_str().unwrap(), c[1].as_str().unwrap()];
            pair.contains(&"one.ipynb") && pair.contains(&"two.ipynb")
        }),
        "{clones:?}"
    );
    assert!(
        !clones.iter().any(|c| {
            let pair = [c[0].as_str().unwrap(), c[1].as_str().unwrap()];
            pair.contains(&"aaa.ipynb") || pair.contains(&"bbb.ipynb")
        }),
        "JSON boilerplate must not pair notebooks: {clones:?}"
    );
    // dup_lines counts code lines: never absurd, never above the file LOC.
    for h in rep["hotspots"].as_array().unwrap() {
        let dup = h.get("dup_lines").and_then(|v| v.as_u64()).unwrap_or(0);
        assert!(dup <= h["loc"].as_u64().unwrap(), "{} dup={dup} loc={}", h["path"], h["loc"]);
    }
}

#[test]
fn sfc_markup_excluded_from_clones() {
    let t = TestDir::new();
    let markup = "<div class=\"card\">\n  <p>Same prose here</p>\n  <span>more words</span>\n</div>\n";
    t.write("sfc/a.vue", &format!("---\n---\n{markup}"));
    t.write("sfc/b.vue", &format!("---\n---\n{markup}"));
    let rep = audit_json(&t.sub("sfc"), &["--top", "5"]);
    assert_eq!(rep["clones"].as_array().unwrap().len(), 0, "markup-only files share no code");
    // Same markup but a shared script block: the script pair is found.
    let js_shared: String = "function shared(a, b) {\n  let total = a + b;\n  if (total > 10) {\n    total = total * 2;\n  }\n  for (let i = 0; i < 3; i++) {\n    total += i;\n  }\n  while (total < 0) {\n    total += 1;\n  }\n  return total;\n}\n"
        .to_string();
    assert!(js_shared.lines().filter(|l| !l.trim().is_empty()).count() >= 8);
    t.write("sfc2/x.vue", &format!("<template><div>different one</div></template>\n<script>\n{js_shared}</script>\n"));
    t.write("sfc2/y.vue", &format!("<template><div>different two</div></template>\n<script>\n{js_shared}</script>\n"));
    let rep2 = audit_json(&t.sub("sfc2"), &["--top", "5"]);
    let clones2 = rep2["clones"].as_array().unwrap();
    assert!(
        clones2.iter().any(|c| {
            let pair = [c[0].as_str().unwrap(), c[1].as_str().unwrap()];
            pair.contains(&"x.vue") && pair.contains(&"y.vue")
        }),
        "{clones2:?}"
    );
}

// 55. Cycle warnings floor the verdict: a repo whose only signal is a
// large tangle must not read LOW/PROCEED (same principle as the HIGH
// floor — PROCEED contradicts "treat the area as one").
#[test]
fn cycle_floors_verdict() {
    let t = TestDir::new();
    for i in 0..10 {
        let nxt = (i + 1) % 10;
        t.write(&format!("ring/m{i}.py"), &format!("from m{nxt} import g{nxt}\n\ndef g{i}(x):\n    return g{nxt}(x)\n"));
    }
    let rep = audit_json(&t.sub("ring"), &[]);
    assert!(rep["warnings"].as_array().unwrap().iter().any(|w| w["msg"].as_str().unwrap().contains("tangle")));
    assert_eq!(rep["verdict"], "MODERATE");
    let (code, stdout, _) = audit(&t.sub("ring"), &[]);
    assert_eq!(code, 0);
    assert!(!stdout.contains("PROCEED"));
}

// 56. Coupling warnings name the risk direction: pure fan-out hubs warn
// about upstream fragility, not ripples (which need importers).
#[test]
fn coupling_direction() {
    let t = TestDir::new();
    for i in 0..25 {
        t.write(&format!("hub/dep{i}.py"), &format!("def v{i}():\n    return {i}\n"));
    }
    let hub = (0..25).map(|i| format!("from dep{i} import v{i}")).collect::<Vec<_>>().join("\n") + "\n\ndef run():\n    return 1\n";
    t.write("hub/cli.py", &hub);
    let rep = audit_json(&t.sub("hub"), &[]);
    let ws: Vec<&Value> = rep["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["msg"].as_str().unwrap().contains("coupling"))
        .collect();
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert_eq!(ws[0]["where"], "cli.py");
    assert!(ws[0]["msg"].as_str().unwrap().contains("outgoing only"), "{}", ws[0]["msg"]);
    assert!(!ws[0]["msg"].as_str().unwrap().contains("ripples"), "{}", ws[0]["msg"]);
}

// 57. Transitive blast radius: a signature change at a chain's bottom
// breaks more than its direct importer; focus names the full reach.
#[test]
fn transitive_dependents() {
    let t = TestDir::new();
    for f in 0..8 {
        let body = if f < 7 {
            format!("from m{} import f{}\n\ndef f{f}(x):\n    if x:\n        return f{}(x)\n    return 0\n", f + 1, f + 1, f + 1)
        } else {
            format!("def f{f}(x):\n    if x:\n        return 1\n    return 0\n")
        };
        t.write(&format!("ch/m{f}.py"), &body);
    }
    let rep = audit_json(&t.sub("ch"), &["--focus", "m7.py"]);
    let f = &rep["focus"];
    assert_eq!(f["n_dependents"], 1);
    assert_eq!(f["n_transitive"], 6);
    let names: Vec<&str> = f["transitive_dependents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    // Top 5 of 6 by coupling (m0 has the lowest); paths root-relative.
    assert!(names.contains(&"m1.py") && names.contains(&"m5.py"), "{names:?}");
    assert!(f["assessment"].as_str().unwrap().contains("transitively"));
    let (code, stdout, _) = audit(&t.sub("ch"), &["--focus", "m7.py"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("TRANSITIVE (6 beyond direct)"), "{stdout}");
}

// 58. Svelte 5 runes are calls, not definitions; markup stays excluded.
#[test]
fn svelte_runes() {
    let t = TestDir::new();
    t.write(
        "runes/app.svelte",
        "<script lang=\"ts\">\n  import { flip } from 'svelte/animate';\n  let { data } = $props();\n  let count = $state(0);\n  let doubled = $derived(count * 2);\n  $effect(() => {\n    if (count > 10) {\n      console.log(doubled);\n    }\n  });\n  function inc() {\n    count += 1;\n  }\n  const reset = () => {\n    count = 0;\n  };\n</script>\n<div>{#if count > 5}<p>many for while if prose?</p>{/if}\n<button onclick={() => inc()}>Hi</button></div>\n",
    );
    let rep = audit_json(&t.sub("runes"), &["--top", "5"]);
    let h = hotspots(&rep);
    let a = get(&h, "app.svelte");
    // inc + reset arrow + $effect arrow; rune calls ($props/$state/$derived)
    // are not definitions; markup if/arrow excluded.
    assert_eq!(a["funcs"], 3);
    assert_eq!(a["complexity"], 5);
    assert_eq!(a["max_func_cx"], 2);
    assert_eq!(a["fan_out_ex"], 1);
    assert_eq!(a["fan_out_in"], 0);
}

// 60. Flat data-like large files warn honestly: dense keeps the HIGH,
// coupled-flat keeps HIGH (ripple is real), isolated-flat drops to WATCH.
#[test]
fn flat_large_file_watch() {
    let t = TestDir::new();
    let mut dense = String::new();
    for i in 0..200 {
        dense.push_str(&format!(
            "def handler_{i}(x, y):\n    if x > {i}:\n        if y:\n            return x\n    return y\n"
        ));
    }
    t.write("flat/dense.py", &dense);
    let mut table = String::from("TABLE = {\n");
    for i in 0..850 {
        table.push_str(&format!("    'k{i:04}': {i},\n"));
    }
    table.push_str("}\n");
    t.write("flat/data_table.py", &table);
    let mut hub = String::from("VALUES = {\n");
    for i in 0..850 {
        hub.push_str(&format!("    'k{i:04}': {i},\n"));
    }
    hub.push_str("}\n");
    t.write("flat/const_hub.py", &hub);
    for i in 0..25 {
        t.write(
            &format!("flat/cons{i:02}.py"),
            "from const_hub import VALUES\nprint(VALUES)\n",
        );
    }
    let rep = audit_json(&t.sub("flat"), &["--top", "30"]);
    let warns: Vec<(&str, &str, &str)> = rep["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| {
            let m = w["msg"].as_str().unwrap();
            if m.starts_with("large file") || m.starts_with("large flat file") {
                Some((w["where"].as_str().unwrap(), w["severity"].as_str().unwrap(), m))
            } else {
                None
            }
        })
        .collect();
    let dense_w = warns.iter().find(|(p, _, _)| p.contains("dense.py")).unwrap();
    assert_eq!(dense_w.1, "HIGH", "{warns:?}");
    let hub_w = warns.iter().find(|(p, _, _)| p.contains("const_hub.py")).unwrap();
    assert_eq!(hub_w.1, "HIGH", "{warns:?}");
    let flat_w = warns.iter().find(|(p, _, _)| p.contains("data_table.py")).unwrap();
    assert_eq!(flat_w.1, "WATCH", "{warns:?}");
    assert!(flat_w.2.contains("cheap per-line"), "{warns:?}");
}

// 59. Pathological entries never hang or miscount: FIFOs/sockets are not
// regular files (skipped silently, like symlinked dirs they cannot score);
// unicode names audit normally.
#[test]
#[cfg(unix)]
fn pathological_entries() {
    use std::os::unix::net::UnixListener;
    let t = TestDir::new();
    t.write("patho/ok.py", "def f():\n    return 1\n");
    t.write("patho/\u{00fc}n\u{00ef}code.py", "def w():\n    return 2\n");
    let fifo = t.sub("patho/pipe.fifo");
    assert_eq!(
        std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().code(),
        Some(0)
    );
    let _sock = UnixListener::bind(t.sub("patho/sock.ts")).unwrap();
    let rep = audit_json(&t.sub("patho"), &["--top", "5"]);
    assert_eq!(rep["files_total"], 2);
    assert_eq!(rep["code_files"], 2);
    let paths: Vec<&str> = rep["hotspots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"ok.py") && paths.iter().any(|p| p.contains("n\u{00ef}code.py")), "{paths:?}");
}

// 61. Annotated/generic TS definitions split like plain ones; overload
// signatures (no body) stay invisible.
#[test]
fn ts_annotated_defs() {
    let t = TestDir::new();
    let body = "{\n  if (a) {\n    return 1;\n  }\n  for (let i = 0; i < 2; i++) {\n    if (i) { return 2; }\n  }\n  return 0;\n}\n";
    t.write("ann/plain.ts", &format!("export function plain(a) {body}"));
    t.write(
        "ann/annot.ts",
        &format!("export function annot(a: number): number {body}"),
    );
    t.write(
        "ann/gen.ts",
        &format!("export function gen<T extends object>(a: T): T {body}"),
    );
    t.write(
        "ann/meth.ts",
        &format!("class C {{\n  run(a: string): number {body}}}\n"),
    );
    // Overload signatures: no bodies, must not count.
    t.write(
        "ann/over.ts",
        "export function over(a: number): number;\nexport function over(a: string): string;\nexport function over(a: unknown): unknown {\n  if (typeof a === 'number') { return 1; }\n  return 0;\n}\n",
    );
    let rep = audit_json(&t.sub("ann"), &["--top", "10"]);
    let h = hotspots(&rep);
    let plain = get(&h, "plain.ts");
    let annot = get(&h, "annot.ts");
    let gen = get(&h, "gen.ts");
    assert_eq!(annot["max_func_cx"], plain["max_func_cx"], "{annot:?} vs {plain:?}");
    assert_eq!(gen["max_func_cx"], plain["max_func_cx"]);
    assert_eq!(annot["funcs"], 1);
    assert_eq!(gen["funcs"], 1);
    let over = get(&h, "over.ts");
    // Overload signatures are declarations: one real function, one body
    // (the old keyword count of 3 was a heuristic artifact).
    assert_eq!(over["funcs"], 1, "{over:?}");
    assert_eq!(over["max_func_cx"], 2, "{over:?}");
    // ASI-style overloads (no semicolons): signatures must not resolve
    // into the implementation body (no duplicate spans, no method
    // inflation).
    t.write(
        "ann/asi.ts",
        "class C {\n  foo(x: number): void\n  foo(x: string): void\n  foo(x: unknown): void {\n    if (typeof x === 'number') { return 1 }\n    return 0\n  }\n}\n",
    );
    let rep2 = audit_json(&t.sub("ann"), &["--top", "10"]);
    let h2 = hotspots(&rep2);
    let asi = get(&h2, "asi.ts");
    assert_eq!(asi["funcs"], 1, "{asi:?}");
    assert_eq!(asi["max_func_cx"], 2, "{asi:?}");
}

// 62. File totals count each branch once: flat and nested twins with
// identical decisions agree (whole-stack max_func_cx is unaffected).
#[test]
fn nested_once_totals() {
    let t = TestDir::new();
    let mut flat = String::new();
    for i in 0..10 {
        flat.push_str(&format!(
            "def f{i}(x):\n    if x > {i}:\n        return 1\n    if x < 0:\n        return 2\n    return 0\n"
        ));
    }
    t.write("nest/flat.py", &flat);
    let mut nested = String::from("def outer(x):\n");
    for i in 0..10 {
        nested.push_str(&format!(
            "    def inner{i}(y):\n        if y > {i}:\n            return 1\n        if y < 0:\n            return 2\n        return 0\n"
        ));
    }
    nested.push_str("    return 0\n");
    t.write("nest/nested.py", &nested);
    let rep = audit_json(&t.sub("nest"), &["--top", "5"]);
    let h = hotspots(&rep);
    let f = get(&h, "flat.py");
    let n = get(&h, "nested.py");
    // Same 20 branches; nested adds exactly the one outer-def unit.
    assert_eq!(f["complexity"], 31);
    assert_eq!(n["complexity"], 32);
    // Whole-stack per-function semantics preserved.
    assert_eq!(n["max_func_cx"], 21);
    assert_eq!(f["max_func_cx"], 3);
}

// 63. Quote/comment stripping is single-state: an apostrophe inside a
// template or double-quoted string must not pair with a later lone quote
// and swallow code; `//` prose never counts even with odd quotes before
// it on the line.
#[test]
fn lex_strip_no_cross_pairing() {
    let t = TestDir::new();
    t.write(
        "lex/a.ts",
        "const a = `it's`;\nfunction foo(x: number): number {\n  if (x) { return 1; }\n  return 0;\n}\nconst b = `don't`;\nfunction bar(y: number): number {\n  if (y) { return 2; }\n  return 0;\n}\nconst s = \"it's\"; // if here\n",
    );
    let rep = audit_json(&t.sub("lex"), &["--top", "5"]);
    let h = hotspots(&rep);
    let a = get(&h, "a.ts");
    // Both functions survive (2 x (1 + if)) + module; prose `if` cut.
    assert_eq!(a["funcs"], 2, "{a:?}");
    assert_eq!(a["complexity"], 5, "{a:?}");
    assert_eq!(a["max_func_cx"], 2, "{a:?}");
}

// 64. Rust analysis: twin parity with Python economics, match/?/boolop
// branches, cfg(test) subtrees unscored, syntax errors located.
#[test]
fn rust_basics() {
    let t = TestDir::new();
    let mut flat = String::new();
    for i in 0..10 {
        flat.push_str(&format!(
            "fn f{i}(x: i32) -> i32 {{\n    if x > {i} {{ return 1; }}\n    if x < 0 {{ return 2; }}\n    0\n}}\n"
        ));
    }
    t.write("rs/flat.rs", &flat);
    let mut nested = String::from("fn outer(x: i32) -> i32 {\n");
    for i in 0..10 {
        nested.push_str(&format!(
            "    let f{i} = |y: i32| {{\n        if y > {i} {{ return 1; }}\n        if y < 0 {{ return 2; }}\n        0\n    }};\n"
        ));
    }
    nested.push_str("    0\n}\n");
    t.write("rs/nested.rs", &nested);
    t.write(
        "rs/lib.rs",
        "use crate::a::{b, c};\nuse std::collections::HashMap;\n\npub fn run(v: Option<i32>) -> i32 {\n    let v = v?;\n    match v {\n        1 => 10,\n        2 | 3 => 20,\n        _ => 0,\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn it_works() {\n        let mut x = 0;\n        for i in 0..10 {\n            if i > 1 && i < 9 || i == 5 { x += i; }\n        }\n        assert_eq!(x, 1);\n    }\n}\n",
    );
    t.write("rs/broken.rs", "fn f( {\n");
    let rep = audit_json(&t.sub("rs"), &["--top", "10"]);
    let h = hotspots(&rep);
    // Same economics as the Python twins (test 62).
    assert_eq!(get(&h, "flat.rs")["complexity"], 31);
    assert_eq!(get(&h, "nested.rs")["complexity"], 32);
    assert_eq!(get(&h, "nested.rs")["max_func_cx"], 21);
    // run: 1 + ? + 3 arms; or-pattern adds nothing; cfg(test) invisible.
    let lib = get(&h, "lib.rs");
    assert_eq!(lib["complexity"], 6, "{lib:?}");
    assert_eq!(lib["funcs"], 1, "{lib:?}");
    assert_eq!(lib["max_func_cx"], 5, "{lib:?}");
    let names: Vec<&str> = lib["top_funcs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["run"], "{names:?}");
    assert!(!rep["parse_errors"].as_array().unwrap().is_empty());
}

// 65. Rust `use` edges resolve onto the module tree: crate/super/self
// paths, braced lists (every part, not just the first), globs; rings
// cycle; ambiguous (foo.rs + foo/mod.rs) and extern heads draw nothing.
#[test]
fn rust_use_edges() {
    let t = TestDir::new();
    t.write("rsedge/Cargo.toml", "[package]\nname = \"edge\"\n");
    t.write("rsedge/src/main.rs", "mod a;\nmod b;\nmod hub;\nmod c1;\nmod c2;\nmod ext;\nmod dup;\nmod amb;\nfn main() {}\n");
    // Ring via crate paths.
    t.write("rsedge/src/a.rs", "use crate::b::get_b;\npub fn get_a() -> i32 { get_b() }\n");
    t.write("rsedge/src/b.rs", "use crate::a::get_a;\npub fn get_b() -> i32 { get_a() }\n");
    // Hub with three consumers: plain + super + braced-alias, plus a
    // second hub hit only via a braced list's SECOND part (brace-aware
    // `::` splitting regression: naive splitting fractures `{a::b}`).
    t.write("rsedge/src/hub.rs", "pub fn core() -> i32 { 1 }\npub fn other() -> i32 { 2 }\n");
    t.write("rsedge/src/hub2.rs", "pub fn core2() -> i32 { 2 }\n");
    t.write("rsedge/src/c1.rs", "use crate::hub::core;\npub fn f() -> i32 { core() }\n");
    t.write(
        "rsedge/src/c2.rs",
        "use super::hub::core;\nuse crate::{hub::core as c, hub2::core2};\npub fn g() -> i32 { core() + c() + core2() }\n",
    );
    // One braced list naming two items from ONE module = one edge, not two.
    t.write("rsedge/src/c3.rs", "use crate::{hub::core, hub::other};\npub fn h() -> i32 { core() + other() }\n");
    // Extern head: no edge. Ambiguous dup.rs + dup/mod.rs: no edge.
    t.write("rsedge/src/ext.rs", "use serde::Serialize;\npub fn gone() {}\n");
    t.write("rsedge/src/dup.rs", "pub fn thing() -> i32 { 1 }\n");
    t.write("rsedge/src/dup/mod.rs", "pub fn thing() -> i32 { 2 }\n");
    t.write("rsedge/src/amb.rs", "use crate::dup::thing;\npub fn h() -> i32 { thing() }\n");
    let rep = audit_json(&t.sub("rsedge"), &["--top", "20"]);
    let h = hotspots(&rep);
    // Ring detected (eager pair).
    let cyc: Vec<Vec<String>> = rep["cycles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            c[0].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect()
        })
        .collect();
    assert!(
        cyc.iter().any(|m| m.contains(&"src/a.rs".to_string()) && m.contains(&"src/b.rs".to_string())),
        "{cyc:?}"
    );
    // Hub: all three consumer imports (braced parts included); c3's
    // two-names-one-module list counts once.
    assert_eq!(get(&h, "src/hub.rs")["fan_in"], 4, "{h:?}");
    assert_eq!(get(&h, "src/hub2.rs")["fan_in"], 1, "{h:?}");
    // Extern miss + ambiguity: no edges, no fan_in.
    assert_eq!(get(&h, "src/ext.rs")["fan_in"], 0);
    assert_eq!(get(&h, "src/dup.rs")["fan_in"], 0);
    // c2's `ext::gone` part is extern (no edge); files exist so hotspots hit.
    let _ = get(&h, "src/c2.rs");
}

// 66. Leading-dot fractions with zero-led fractions (`.075`) parse;
// genuine malformed numbers still error exactly where CPython blames.
#[test]
fn leading_dot_zero_fraction() {
    let t = TestDir::new();
    t.write(
        "num/ok.py",
        "a = .075\nb = [.2, .1, .1, .075]\nc = .5_5\nd = 001.5\ne = 0x_1\n",
    );
    let rep = audit_json(&t.sub("num"), &["--top", "5"]);
    assert!(rep["parse_errors"].as_array().unwrap().is_empty());
    let h = hotspots(&rep);
    // Module-level assignments carry no branches; the point is it parses
    // with metrics instead of falling back to line counts.
    assert_eq!(get(&h, "ok.py")["complexity"], 0, "{h:?}");
    for (name, body) in [
        ("e1.py", "x = 0755\n"),
        ("e2.py", "x = 1j2\n"),
        ("e3.py", "x = 0x1G\n"),
        ("e4.py", "x = 1__2\n"),
    ] {
        t.write(&format!("num/{name}"), body);
    }
    let rep2 = audit_json(&t.sub("num"), &["--top", "5"]);
    assert_eq!(rep2["parse_errors"].as_array().unwrap().len(), 4);
}

// 67. Focus cycle display carries size and kind: an eager pair and a lazy
// pair with identical truncated prefixes read differently.
#[test]
fn focus_cycle_labels() {
    let t = TestDir::new();
    t.write("cy/eager_a.py", "from eager_b import g\n\ndef f():\n    return g()\n");
    t.write("cy/eager_b.py", "from eager_a import f\n\ndef g():\n    return f()\n");
    t.write(
        "cy/lazy_a.py",
        "def f():\n    from lazy_b import g\n    return g()\n",
    );
    t.write(
        "cy/lazy_b.py",
        "def g():\n    from lazy_a import f\n    return f()\n",
    );
    let rep = audit_json(&t.sub("cy"), &["--focus", "eager_a.py"]);
    let f = &rep["focus"];
    let a = f["assessment"].as_str().unwrap();
    assert!(a.contains("2-module cycle"), "{a}");
    assert!(!a.contains("lazy"), "{a}");
    let rep2 = audit_json(&t.sub("cy"), &["--focus", "lazy_a.py"]);
    let a2 = rep2["focus"]["assessment"].as_str().unwrap();
    assert!(a2.contains("2-module lazy tangle"), "{a2}");
    let kinds: Vec<bool> = rep2["focus"]["cycles_through"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["eager"].as_bool().unwrap())
        .collect();
    assert_eq!(kinds, vec![false]);
}

// 68. Compound extensions resolve exactly: `./cache.svelte.js` hits the
// .svelte.js file, not the stem-sharing `cache.js` via tail fallback.
#[test]
fn compound_extension_resolution() {
    let t = TestDir::new();
    t.write(
        "cx/a.js",
        "import { x } from './cache.svelte.js';\nimport { y } from './cache.js';\nconsole.log(x, y);\n",
    );
    t.write("cx/cache.svelte.js", "export const x = 1;\n");
    t.write("cx/cache.js", "export const y = 2;\n");
    // Audit the parent: the files must sit in a subdirectory so module
    // resolution has directory context (root-level files cannot
    // full-match by construction).
    let rep = audit_json(&t.path, &["--top", "5"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "cx/cache.svelte.js")["fan_in"], 1, "{h:?}");
    assert_eq!(get(&h, "cx/cache.js")["fan_in"], 1, "{h:?}");
}

// 69. Rebuilt-output headers ("do not edit" + "rebuild" across header
// lines) are verdict-neutral; either half alone stays production.
#[test]
fn rebuilt_header_neutral() {
    let t = TestDir::new();
    t.write(
        "rb/schema.py",
        "# DO NOT EDIT THIS FILE DIRECTLY.\n# INSTEAD, edit types.ts\n# THEN RUN build:schema TO REBUILD\n\nMODELS = {}\n",
    );
    // Only the rebuild half: production.
    t.write("rb/tool.py", "# Rebuild the index with `make index`.\n\ndef build():\n    return 1\n");
    // Only the do-not-edit half (section marker): production.
    t.write("rb/cfg.py", "# Do not edit below this line.\n\nLIMIT = 10\n");
    let rep = audit_json(&t.sub("rb"), &["--top", "8"]);
    let h = hotspots(&rep);
    assert_eq!(rep["prod_files"], 2, "{rep:?}");
    assert!(h.contains_key("tool.py") && h.contains_key("cfg.py"), "{h:?}");
    assert!(!h.contains_key("schema.py"), "{h:?}");
}

// 70. Verdict honesty under unscored-language dominance: a repo whose code
// is overwhelmingly an unsupported language reads N/A (not a verdict over
// the scored sliver); balanced repos are unaffected.
#[test]
fn scope_dominance_na() {
    let t = TestDir::new();
    for i in 0..30 {
        t.write(&format!("dom/pkg{}.go", i), "package p\n\nfunc F() int {\n\treturn 1\n}\n");
    }
    t.write("dom/help.py", "def help():\n    return 1\n");
    let rep = audit_json(&t.sub("dom"), &["--top", "5"]);
    assert_eq!(rep["verdict"], "N/A");
    assert!(rep["verdict_why"].as_str().unwrap().contains("dominated by unscored .go"));
    assert!(rep["guidance"].as_array().unwrap().iter().any(|g| g.as_str().unwrap().contains("OUT OF SCOPE")));
    // Balanced: 3 Go + 3 Python stays judged.
    let t2 = TestDir::new();
    for i in 0..3 {
        t2.write(&format!("mix/pkg{}.go", i), "package p\n");
    }
    t2.write("mix/a.py", "def f(x):\n    if x:\n        return 1\n    return 0\n");
    let rep2 = audit_json(&t2.sub("mix"), &["--top", "5"]);
    assert_ne!(rep2["verdict"], "N/A");
}

// 73. Nested src-layout tops: multi-distribution monorepos
// (`dista/src/acme`, `distb/src/acme`) resolve absolute `acme.*`
// imports (hub fan_in counts both distributions' importers).
// Namespace stubs without `__init__.py` stay missed (status quo).
#[test]
fn nested_src_toplevels() {
    let t = TestDir::new();
    t.write("mono/dista/src/acme/__init__.py", "");
    t.write("mono/dista/src/acme/hub.py", "def hub():\n    return 1\n");
    t.write(
        "mono/dista/src/acme/leaf.py",
        "from acme.hub import hub\n\ndef leaf():\n    return hub()\n",
    );
    t.write("mono/distb/src/acme/__init__.py", "");
    t.write(
        "mono/distb/src/acme/extra.py",
        "from acme.hub import hub\n\ndef extra():\n    return hub()\n",
    );
    t.write(
        "mono/app.py",
        "from acme.leaf import leaf\n\nprint(leaf())\n",
    );
    // No `__init__.py` under nosrc/src/nopkg/: still missed.
    t.write("mono/nosrc/src/nopkg/mod.py", "def f():\n    return 1\n");
    t.write(
        "mono/use_nopkg.py",
        "from nopkg.mod import f\n\nprint(f())\n",
    );
    let rep = audit_json(&t.sub("mono"), &["--top", "10"]);
    let h = hotspots(&rep);
    assert_eq!(get(&h, "dista/src/acme/hub.py")["fan_in"], 2);
    assert_eq!(get(&h, "dista/src/acme/leaf.py")["fan_in"], 1);
    assert_eq!(get(&h, "nosrc/src/nopkg/mod.py")["fan_in"], 0);
}

// 71. Focus hotspot imperative is earned: trivial overlap lists without a
// test-cover demand (tiny repos put every file in top-N); dense overlap
// keeps it. Same nonsense family as the test-focus fix (#41).
#[test]
fn focus_hotspot_imperative_earned() {
    let t = TestDir::new();
    let mut dense = String::from("def core(x):\n");
    for i in 0..30 {
        dense.push_str(&format!("    if x == {i}:\n        y = {i}\n"));
    }
    dense.push_str("    return y\n");
    t.write("sm/dense.py", &dense);
    t.write("sm/trivial.py", "CONST = 1\n\ndef get():\n    return CONST\n");
    t.write("sm/empty.py", "X = 1\n");
    // Trivial focus: cheap verdict stands, hotspot named, no test demand.
    let rep = audit_json(&t.sub("sm"), &["--focus", "trivial.py"]);
    let a = rep["focus"]["assessment"].as_str().unwrap();
    assert!(a.contains("Cheapest place"), "{a}");
    assert!(a.contains("trivial.py"), "{a}");
    assert!(!a.contains("cover with tests"), "{a}");
    let rep = audit_json(&t.sub("sm"), &["--focus", "empty.py"]);
    let a = rep["focus"]["assessment"].as_str().unwrap();
    assert!(!a.contains("cover with tests"), "{a}");
    // Dense focus: expensive verdict names the hotspot WITH the demand.
    let rep = audit_json(&t.sub("sm"), &["--focus", "dense.py"]);
    let a = rep["focus"]["assessment"].as_str().unwrap();
    assert!(a.contains("expensive code"), "{a}");
    assert!(a.contains("cover with tests"), "{a}");
}

// Bare top-level import must resolve the package root, never an unrelated
// same-stem file (regression: `import django` once drew an edge to
// template/backends' django.py, welding warnings.py into a 44-module
// cycle; package execution semantics demand the __init__ target).
#[test]
fn bare_top_import_prefers_package_root() {
    let t = TestDir::new();
    t.write("pkg/__init__.py", "X = 1\n");
    t.write("pkg/w.py", "import pkg\nprint(pkg.X)\n");
    t.write("pkg/sub/pkg.py", "X = 2\n");
    // The same-stem module must have no phantom importer ...
    let rep = audit_json(t.path.as_path(), &["--focus", "pkg/sub/pkg.py"]);
    assert_eq!(rep["focus"]["n_dependents"], 0);
    // ... while the package root names the real one.
    let rep = audit_json(t.path.as_path(), &["--focus", "pkg/__init__.py"]);
    let deps: Vec<String> = rep["focus"]["dependents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.get("path").and_then(|p| p.as_str()).map(str::to_string))
        .collect();
    assert!(deps.iter().any(|p| p == "pkg/w.py"), "{deps:?}");
    // No cycle anywhere in this fixture.
    let rep = audit_json(t.path.as_path(), &[]);
    assert!(rep["cycles"].as_array().unwrap().is_empty());
}

// `from pkg import submodule` draws an edge to the submodule file; a
// function name never becomes a phantom edge to a same-named file.
#[test]
fn py_from_import_submodule_edges() {
    let t = TestDir::new();
    t.write("sub/app/__init__.py", "");
    t.write("sub/app/crud.py", "def get():\n    return 1\n");
    t.write("sub/app/helpers.py", "def h():\n    return 2\n");
    t.write("sub/app/api/__init__.py", "");
    t.write("sub/app/api/users.py", "from app import crud\n\ndef u():\n    return crud.get()\n");
    t.write("sub/app/api/items.py", "from .. import helpers\n\ndef i():\n    return helpers.h()\n");
    t.write("sub/app/api/orders.py", "from app.crud import get\n\ndef o():\n    return get()\n");
    t.write("sub/other/get.py", "def x():\n    return 3\n");
    let deps = |target: &str| -> Vec<String> {
        let rep = audit_json(&t.sub("sub"), &["--focus", target]);
        rep["focus"]["dependents"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["path"].as_str().map(str::to_string))
            .collect()
    };
    let crud = deps("app/crud.py");
    assert!(crud.contains(&"app/api/users.py".to_string()), "{crud:?}");
    assert!(crud.contains(&"app/api/orders.py".to_string()), "{crud:?}");
    assert!(deps("app/helpers.py").contains(&"app/api/items.py".to_string()));
    assert!(deps("other/get.py").is_empty(), "phantom edge from a function name");
}
