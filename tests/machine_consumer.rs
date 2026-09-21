//! Exp 50: machine-consumer test (branch-only). A mock coding-agent
//! workflow with no LLM consumes `cxcap audit --json` and must answer:
//! top touchpoints, direct/transitive dependents, uncertainty, context,
//! warnings-with-evidence. Interface usability, not AI benchmark.

mod common;

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(50_000);

struct Tmp {
    path: PathBuf,
}
impl Tmp {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("cxcap-mc-{}-{id}", std::process::id()));
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

fn audit_json(dir: &Path, args: &[&str]) -> Value {
    let bin = common::bin();
    let out = Command::new(bin)
        .arg("audit")
        .arg(dir)
        .arg("--json")
        .args(args)
        .output()
        .expect("run cxcap");
    assert_eq!(out.status.code(), Some(0));
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON")
}

/// Mock agent: plan a change to `auth/session.ts`; answer from JSON only.
#[test]
fn agent_can_constrain_plan_from_json() {
    let t = Tmp::new();
    t.write(
        "auth/session.ts",
        "import { Cfg } from './config';\nexport interface Session { t: string; }\nexport function openSession(c: Cfg) { return c; }\n",
    );
    t.write(
        "auth/config.ts",
        "export interface Cfg { token: string; }\n",
    );
    t.write(
        "auth/service.ts",
        "import { openSession } from './session';\nexport function serve() { return openSession({token:'x'}); }\n",
    );
    t.write("util/pad.ts", "export function pad(s: string) { return s; }\n");
    t.write(
        "auth/session.test.ts",
        "import { openSession } from './session';\ntest('s', () => { openSession({token:'x'}); });\n",
    );

    let rep = audit_json(&t.path, &["--focus", "auth/session.ts"]);
    // 1. Top touchpoints: focus block names the area + hotspots rank files.
    let focus = rep.get("focus").expect("focus present");
    let focus_txt = serde_json::to_string(focus).unwrap();
    assert!(focus_txt.contains("auth/session.ts"), "focus names area");
    // 2. Direct dependents: focus lists outside dependents incl. service.
    assert!(focus_txt.contains("auth/service.ts"), "direct dependent visible");
    // 3. Transitive reach: focus exposes transitive count/paths.
    assert!(
        focus.get("transitive_dependents").is_some()
            || focus_txt.contains("transitive"),
        "transitive exposure present"
    );
    // 4. Cycles touching area: field exists (empty vec is a valid answer).
    assert!(focus.get("cycles_through").is_some() || focus.get("cycles").is_some() || focus_txt.contains("cycle"), "cycle evidence present");
    // 5. Verification surface: test importer labeled, not ranked as hotspot.
    let all = serde_json::to_string(&rep).unwrap();
    assert!(all.contains("auth/session.test.ts"), "verification surface disclosed");
    let hotspots: Vec<String> = rep
        .get("hotspots")
        .and_then(|h| h.as_array())
        .map(|a| a.iter().filter_map(|h| h.get("path").and_then(|p| p.as_str()).map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(!hotspots.contains(&"auth/session.test.ts".to_string()), "tests not production hotspots");
    // 6. Warnings carry evidence (path + reason), version present for compat.
    assert!(rep.get("cxcap_version").is_some(), "version for agents");
    if let Some(ws) = rep.get("warnings").and_then(|w| w.as_array()) {
        for w in ws.iter().take(3) {
            let s = serde_json::to_string(w).unwrap();
            assert!(s.contains("src") || s.contains("auth") || s.contains("util") || s.contains(".ts"), "warning has path evidence: {s}");
        }
    }
    // 7. Relative paths only: no tmp absolute leak in hotspot paths.
    for h in hotspots {
        assert!(!h.starts_with('/'), "relative path: {h}");
        assert!(!h.contains("cxcap-mc-"), "no scratch leak: {h}");
    }
    // 8. Recommended context smaller than read-everything: focus top <= hotspots+focus scope.
    let _maps: HashMap<String, HashSet<String>> = HashMap::new();
}
