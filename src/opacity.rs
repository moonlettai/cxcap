//! Static-analysis uncertainty / opacity flags.
//!
//! Reports WHEN the dependency graph may be incomplete (dynamic dispatch,
//! runtime registries), never as complexity. Epistemic warnings only:
//! "blast radius may be incomplete". Each detector is a high-precision
//! generic substring over file text; noisy patterns are excluded.

#[derive(Debug, Clone, PartialEq)]
pub struct OpacityFlag {
    /// Stable detector id, e.g. "dynamic-import".
    pub kind: &'static str,
    pub detail: String,
}

/// Flag dynamic/opaque mechanisms in one file's text.
/// `lang`: "python" | "js" | "rust" | other (rust yields no flags yet).
pub fn flags_for_text(text: &str, lang: &str) -> Vec<OpacityFlag> {
    match lang {
        "js" => flags_for_js(text),
        "python" => flags_for_python(text),
        _ => Vec::new(),
    }
}

fn flags_for_js(text: &str) -> Vec<OpacityFlag> {
    let mut out = Vec::new();
    if text.contains("import(") {
        out.push(OpacityFlag { kind: "dynamic-import", detail: "dynamic import() present; static edges may miss targets".into() });
    }
    if text.contains("eval(") || text.contains("new Function(") {
        out.push(OpacityFlag { kind: "runtime-eval", detail: "runtime code evaluation; static relations incomplete".into() });
    }
    // Non-literal require: require(x) where x is not a quote/backtick.
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            continue;
        }
        if let Some(i) = t.find("require(") {
            let after = &t[i + "require(".len()..];
            let c = after.chars().next().unwrap_or('"');
            if c != '"' && c != '\'' && c != '`' {
                out.push(OpacityFlag { kind: "dynamic-require", detail: "non-literal require(); static target unknown".into() });
                break;
            }
        }
    }
    out
}

fn flags_for_python(text: &str) -> Vec<OpacityFlag> {
    let mut out = Vec::new();
    if text.contains("__import__(") || text.contains("importlib.") {
        out.push(OpacityFlag { kind: "dynamic-import", detail: "runtime import machinery; static edges may miss targets".into() });
    }
    if text.contains("eval(") || text.contains("exec(") {
        out.push(OpacityFlag { kind: "runtime-eval", detail: "runtime code evaluation; static relations incomplete".into() });
    }
    if text.contains("getattr(") {
        out.push(OpacityFlag { kind: "dynamic-lookup", detail: "getattr() dispatch; static call targets unknown".into() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_dynamic_import_flagged_static_clean() {
        assert!(flags_for_text("const m = await import('./x');", "js")
            .iter().any(|f| f.kind == "dynamic-import"));
        assert!(flags_for_text("import { a } from './x';\nimport './y';", "js").is_empty());
    }

    #[test]
    fn js_require_literal_clean_dynamic_flagged() {
        assert!(flags_for_text("const x = require('./x');", "js").is_empty());
        assert!(flags_for_text("const x = require(name);", "js")
            .iter().any(|f| f.kind == "dynamic-require"));
        // Comment prose does not flag.
        assert!(flags_for_text("// we require patience here", "js").is_empty());
    }

    #[test]
    fn py_detectors_precise() {
        assert!(flags_for_text("m = __import__(name)", "python")
            .iter().any(|f| f.kind == "dynamic-import"));
        assert!(flags_for_text("import importlib\nimportlib.import_module(n)", "python")
            .iter().any(|f| f.kind == "dynamic-import"));
        assert!(flags_for_text("getattr(obj, name)()", "python")
            .iter().any(|f| f.kind == "dynamic-lookup"));
        assert!(flags_for_text("x = 1\ny = x + 2", "python").is_empty());
        // 'evaluate' prose has no 'eval(' so stays clean.
        assert!(flags_for_text("# evaluate the model", "python").is_empty());
    }

    #[test]
    fn uncertainty_is_not_complexity() {
        // Flags carry no numeric score by construction.
        let f = flags_for_text("eval(x)", "js");
        assert!(!f.is_empty());
        let _ = format!("{f:?}"); // only kind+detail, no score field exists
    }
}
