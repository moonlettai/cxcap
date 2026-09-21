//! Embedded-source extraction: notebooks and single-file components.
//!
//! Pulls analyzable script text out of wrapper formats (notebook JSON,
//! frontmatter plus script blocks) so scorers hash and measure code,
//! never markup or boilerplate.

use crate::text::splitlines;

// --- notebook extraction ---

/// Extract Python source from a .ipynb JSON document. None when the
/// document isn't valid notebook JSON. Drops line-magics/shell escapes.
pub fn notebook_source(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let cells = v.get("cells")?.as_array()?;
    let mut out: Vec<String> = Vec::new();
    for cell in cells {
        // Non-dict cells are skipped (reference: isinstance check → continue).
        let Some(obj) = cell.as_object() else {
            continue;
        };
        if obj.get("cell_type")?.as_str()? != "code" {
            continue;
        }
        let src = obj.get("source").cloned().unwrap_or(serde_json::Value::Null);
        let joined: String = if let Some(s) = src.as_str() {
            s.to_string()
        } else if let Some(arr) = src.as_array() {
            let mut j = String::new();
            for item in arr {
                j.push_str(item.as_str()?);
            }
            j
        } else {
            // Missing source behaves like [] (join of nothing); any other
            // non-string element is not valid notebook JSON for our purpose.
            // The reference joins `src` directly: non-list non-str would raise
            // TypeError, which is NOT caught (only ValueError/AttributeError).
            // Mirror: treat null/missing as empty, anything else as invalid.
            if src.is_null() {
                String::new()
            } else {
                return None;
            }
        };
        for ln in splitlines(&joined) {
            let s = ln.trim();
            if s.starts_with(['%', '!', '?']) || s.ends_with('?') {
                continue;
            }
            out.push(ln.to_string());
        }
    }
    let mut res = out.join("\n");
    res.push('\n');
    Some(res)
}

// --- SFC extraction ---

fn frontmatter(text: &str) -> Option<&str> {
    // \A---\s*\n(.*?)\n---\s*\n  (DOTALL)
    // Line-oriented: the opening fence is the first line, the closing
    // fence the first later `---`-plus-blanks line. Both fences must be
    // newline-terminated and the closing fence preceded by a newline, so
    // byte offsets slice the original exactly (matter excludes the
    // newline before the closing fence).
    let mut lines = text.split_inclusive('\n');
    let first = lines.next()?;
    let blanks = first.strip_prefix("---")?.strip_suffix('\n')?;
    if !blanks.trim_matches([' ', '\t']).is_empty() {
        return None;
    }
    let mut off = first.len();
    let begin = off;
    for line in lines {
        // Closing fence: `---` plus blanks only, newline-terminated, and
        // preceded by a newline (off > begin) — mirroring the reference's
        // \n--- + blanks + \n exactly.
        if off > begin && line.ends_with('\n') {
            let content = &line[..line.len() - 1];
            if content.as_bytes().starts_with(b"---")
                && content[3..].trim_matches([' ', '\t']).is_empty()
            {
                // off - 1: drop the newline ending the matter.
                return Some(&text[begin..off - 1]);
            }
        }
        off += line.len();
    }
    None
}

fn script_blocks(text: &str) -> Vec<&str> {
    // <script[^>]*>(.*?)</script>  (DOTALL, CASELESS, non-greedy, leftmost).
    // Case is folded on a lowercased copy for searching; ASCII folding
    // preserves byte length, so the offsets slice the original directly
    // (bodies keep their original case). [^>]* cannot cross '>'; (.*?)
    // stops at the first case-insensitive </script>.
    let lower = text.to_ascii_lowercase();
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = lower.get(pos..).and_then(|t| t.find("<script")) {
        let s = pos + rel;
        // [^>]*> : scan to next '>'
        let mut j = s + 7;
        while j < b.len() && b[j] != b'>' {
            j += 1;
        }
        if j >= b.len() {
            break;
        }
        let body_start = j + 1;
        // (.*?)</script> : first case-insensitive </script>
        match lower[body_start..].find("</script>") {
            Some(off) => {
                out.push(&text[body_start..body_start + off]);
                pos = body_start + off + 9;
            }
            None => break,
        }
    }
    out
}

/// Pull script logic out of a single-file component. Returns
/// (script_text, found_any).
pub fn sfc_extract(text: &str) -> (String, bool) {
    let mut parts: Vec<&str> = Vec::new();
    let mut found = false;
    if let Some(fm) = frontmatter(text) {
        parts.push(fm);
        found = true;
    }
    for b in script_blocks(text) {
        if !b.trim().is_empty() {
            parts.push(b);
            found = true;
        }
    }
    (parts.join("\n"), found)
}

#[cfg(test)]
mod tests {
    use super::sfc_extract;

    fn cases(items: &[(&str, &str, bool)]) {
        for (input, want_text, want_found) in items {
            assert_eq!(&sfc_extract(input), &((*want_text).to_string(), *want_found), "input {input:?}");
        }
    }

    #[test]
    fn basic_and_case_insensitive_tags() {
        cases(&[
            ("<script>let x = 1;</script>", "let x = 1;", true),
            ("<SCRIPT>let x = 1;</SCRIPT>", "let x = 1;", true),
            ("<ScRiPt>let x = 1;</ScRiPt>", "let x = 1;", true),
            ("<script>code</SCRIPT>", "code", true),
            ("<script lang=\"ts\" setup>code</script>", "code", true),
            ("<SCRIPT SRC=\"x.js\"></SCRIPT>", "", false),
        ]);
    }

    #[test]
    fn missing_or_unclosed_blocks_found_nothing() {
        cases(&[
            ("<div>hi</div>", "", false),
            ("<template><div/></template>", "", false),
            ("<script>code", "", false),
            ("<script", "", false),
            ("code</script>", "", false),
        ]);
    }

    #[test]
    fn empty_blocks_skipped_multiples_joined() {
        cases(&[
            ("<script>   </script><script>code</script>", "code", true),
            ("<script>a</script><script>b</script>", "a\nb", true),
            ("<script>if (a < b) { x(); }</script>", "if (a < b) { x(); }", true),
            ("<script>const s = \"h\u{e9}llo\";</script>", "const s = \"h\u{e9}llo\";", true),
        ]);
    }

    #[test]
    fn frontmatter_combines_with_script() {
        cases(&[(
            "---\ntitle: x\n---\n<script>code</script>",
            "title: x\ncode",
            true,
        )]);
    }

    #[test]
    fn frontmatter_fence_shapes() {
        // Opening fence allows trailing blanks but needs the newline.
        // Closing fence needs \n--- + blanks + \n (EOF without the final
        // newline does not close); ---- is not a fence; CRLF is not
        // newline enough (reference regex is \n-only).
        cases(&[
            ("---\ntitle\n---\n<body>", "title", true),
            ("---   \ntitle\n---\n<body>", "title", true),
            ("---\t\ntitle\n---\n<body>", "title", true),
            ("---title\n---\n", "", false),
            ("---\ntitle\n<body>", "", false),
            ("---\ntitle\n---", "", false),
            ("---\ntitle\n---   \n<body>", "title", true),
            ("---\n---\n<body>", "", false),
            ("---\na\n--- b\n---\nc", "a\n--- b", true),
            ("title\n---\n", "", false),
            ("----\ntitle\n---\n<body>", "", false),
            ("---\na: 1\nb: 2\n---\n<body>", "a: 1\nb: 2", true),
            ("---\r\ntitle\r\n---\r\n<body>", "", false),
        ]);
    }
}
