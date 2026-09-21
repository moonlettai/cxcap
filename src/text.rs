//! Text primitives: line splitting, decoding, and shebang sniffing.
//!
//! Byte-level reading conventions shared by every analyzer: Python-style
//! splitlines, utf-8-sig universal-newline decoding, and Python shebang
//! detection for extensionless executables.

// --- Python str.splitlines() on universal-newline-translated text ---
//
// scan() translates \r\n and \r to \n first (mirroring text-mode reads),
// so the remaining extra boundaries are \x0b \x0c \x1c-\x1e \x85 \u2028\u2029.

fn is_extra_boundary(c: char) -> bool {
    matches!(c, '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// Split like Python str.splitlines(): pieces never include the boundary;
/// a trailing boundary adds no empty piece.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == '\n' || is_extra_boundary(c) {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    if start < s.len() || s.is_empty() && out.is_empty() {
        // Nothing trailing: "a\n" yields ["a"], "" yields [].
        if start < s.len() {
            out.push(&s[start..]);
        }
    }
    out
}

/// Universal-newline translation + BOM strip, mirroring
/// `open(path, encoding="utf-8-sig", errors="replace")` text reads.
pub fn decode_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let t = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    if !t.contains('\r') {
        return t.to_string();
    }
    let mut out = String::with_capacity(t.len());
    let mut it = t.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\r' {
            if it.peek() == Some(&'\n') {
                it.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

// --- shebang sniff ---

/// True for #! lines invoking a Python interpreter, including the
/// `/usr/bin/env python3` form. Reads the already-loaded text's first line
/// (equivalent to the reference's readline(300): first line, ≤300 chars).
pub fn has_py_shebang_text(text: &str) -> bool {
    let first = text.split('\n').next().unwrap_or("");
    let line: String = first.chars().take(300).collect();
    let line = line.trim();
    if !line.starts_with("#!") {
        return false;
    }
    let parts: Vec<&str> = line[2..].trim().split_whitespace().collect();
    if parts.is_empty() {
        return false;
    }
    let prog = parts[0].rsplit('/').next().unwrap_or("");
    let cands: Vec<&str> = if prog == "env" && parts.len() > 1 {
        vec![parts[1].rsplit('/').next().unwrap_or("")]
    } else {
        vec![prog]
    };
    cands.iter().any(|c| is_python_prog(c))
}

fn is_python_prog(c: &str) -> bool {
    // (?:python[w]?[\d.]*|pypy[\d.]*)\Z as a full match.
    let (rest, ok) = if let Some(r) = c.strip_prefix("pypy") {
        (r, true)
    } else if let Some(r) = c.strip_prefix("python") {
        let r = r.strip_prefix('w').unwrap_or(r);
        (r, true)
    } else {
        ("", false)
    };
    ok && rest.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
}
