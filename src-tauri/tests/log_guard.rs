//! Guard for the privacy rule "logs never contain transcript text" (SPEC Section 10).
//!
//! Scans every Rust source file under `src/` and fails if a log or print macro mentions a name that holds
//! transcript text, either as an argument (`info!(text = %t)`, `warn!("{}", segment.original_text)`) or inside the
//! format string (`info!("heard {text}")`).
//!
//! The check is deliberately strict. To log something about transcript text, bind the number first and log that:
//! `let chars = text.chars().count(); info!(chars, "segment finalized");`
//!
//! It is a lint, not a proof: it cannot see text that reaches a log through a differently named variable.
//! Keep transcript text in variables with the names below, and never put it in error messages.

use std::fs;
use std::path::{Path, PathBuf};

/// Macros whose arguments end up in a log file, the console, or a panic message.
const LOG_MACROS: &[&str] = &[
    "trace", "debug", "info", "warn", "error", "println", "eprintln", "print", "eprint", "dbg",
    "panic",
];

/// Names that hold transcript text. Compared case-insensitively, whole word only.
const FORBIDDEN_NAMES: &[&str] = &[
    "text",
    "original_text",
    "edited_text",
    "transcript",
    "transcript_text",
    "segment_text",
    "utterance",
    "caption",
];

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    line: usize,
    macro_name: String,
    name: String,
}

#[derive(Debug, Default)]
struct ScanResult {
    /// Log macro calls inspected. Lets the real test prove the scanner is looking at something.
    macros_seen: usize,
    findings: Vec<Finding>,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A string literal found in the source: where its text is, and where the literal ends.
struct Literal {
    content_start: usize,
    content_end: usize,
    end: usize,
}

/// If a string literal (plain, raw or byte) starts at `i`, describes it.
fn string_literal(chars: &[char], i: usize) -> Option<Literal> {
    let mut j = i;
    if chars.get(j) == Some(&'b') {
        j += 1;
    }
    let raw = chars.get(j) == Some(&'r');
    if raw {
        j += 1;
    }
    let mut hashes = 0;
    if raw {
        while chars.get(j) == Some(&'#') {
            hashes += 1;
            j += 1;
        }
    }
    if chars.get(j) != Some(&'"') {
        return None;
    }
    j += 1;
    let content_start = j;

    if raw {
        while j < chars.len() {
            if chars[j] == '"' && (0..hashes).all(|k| chars.get(j + 1 + k) == Some(&'#')) {
                return Some(Literal {
                    content_start,
                    content_end: j,
                    end: j + 1 + hashes,
                });
            }
            j += 1;
        }
    } else {
        while j < chars.len() {
            match chars[j] {
                '\\' => j += 2,
                '"' => {
                    return Some(Literal {
                        content_start,
                        content_end: j,
                        end: j + 1,
                    });
                }
                _ => j += 1,
            }
        }
    }
    // Unterminated: treat the rest of the file as the literal.
    Some(Literal {
        content_start,
        content_end: chars.len(),
        end: chars.len(),
    })
}

/// If a comment starts at `i`, returns the index just past it.
fn comment_end(chars: &[char], i: usize) -> Option<usize> {
    if chars.get(i) != Some(&'/') {
        return None;
    }
    match chars.get(i + 1) {
        Some('/') => Some(
            (i..chars.len())
                .find(|&j| chars[j] == '\n')
                .unwrap_or(chars.len()),
        ),
        Some('*') => {
            let mut depth = 1;
            let mut j = i + 2;
            while j < chars.len() && depth > 0 {
                if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
                    depth += 1;
                    j += 2;
                } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            Some(j)
        }
        _ => None,
    }
}

/// Skips a character literal such as `'x'` or `'\n'`. A lone `'` (a lifetime) is skipped by one character.
fn char_literal_end(chars: &[char], i: usize) -> usize {
    if chars.get(i + 1) == Some(&'\\') {
        return (i + 2..chars.len())
            .find(|&j| chars[j] == '\'')
            .map_or(chars.len(), |j| j + 1);
    }
    if chars.get(i + 2) == Some(&'\'') {
        return i + 3;
    }
    i + 1
}

/// Identifiers used as `{name}` or `{name:?}` placeholders inside a format string.
fn placeholders(content: &[char]) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    while i < content.len() {
        if content[i] != '{' {
            i += 1;
            continue;
        }
        if content.get(i + 1) == Some(&'{') {
            i += 2; // an escaped brace, not a placeholder
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < content.len() && content[end] != '}' && content[end] != ':' {
            end += 1;
        }
        let name: String = content[start..end].iter().collect();
        let name = name.trim();
        if name.chars().next().is_some_and(is_ident_start) && name.chars().all(is_ident_char) {
            names.push(name.to_string());
        }
        i = end;
    }
    names
}

/// Reads a macro body that opens at `open`. Returns the index after the closing bracket and every name that
/// appears as code or as a format-string placeholder.
fn macro_body(chars: &[char], open: usize) -> (usize, Vec<String>) {
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut i = open;
    while i < chars.len() {
        if let Some(end) = comment_end(chars, i) {
            i = end;
            continue;
        }
        let c = chars[i];
        if c == '\'' {
            i = char_literal_end(chars, i);
            continue;
        }
        if is_ident_start(c) || c == '"' {
            let start = i;
            if c != '"' {
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
            }
            let word: String = chars[start..i].iter().collect();
            let may_prefix_string = c == '"' || matches!(word.as_str(), "r" | "b" | "br");
            if may_prefix_string && let Some(literal) = string_literal(chars, start) {
                names.extend(placeholders(
                    &chars[literal.content_start..literal.content_end],
                ));
                i = literal.end;
                continue;
            }
            names.push(word);
            continue;
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return (i + 1, names);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (chars.len(), names)
}

fn scan(source: &str) -> ScanResult {
    let chars: Vec<char> = source.chars().collect();
    let mut result = ScanResult::default();
    let mut i = 0;
    while i < chars.len() {
        if let Some(end) = comment_end(&chars, i) {
            i = end;
            continue;
        }
        let c = chars[i];
        if c == '\'' {
            i = char_literal_end(&chars, i);
            continue;
        }
        if c.is_ascii_digit() {
            while i < chars.len() && is_ident_char(chars[i]) {
                i += 1;
            }
            continue;
        }
        if c == '"' || is_ident_start(c) {
            let start = i;
            if c != '"' {
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
            }
            let word: String = chars[start..i].iter().collect();
            let may_prefix_string = c == '"' || matches!(word.as_str(), "r" | "b" | "br");
            if may_prefix_string && let Some(literal) = string_literal(&chars, start) {
                i = literal.end;
                continue;
            }
            if LOG_MACROS.contains(&word.as_str()) {
                let mut j = i;
                while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                    j += 1;
                }
                if chars.get(j) == Some(&'!') {
                    j += 1;
                    while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                        j += 1;
                    }
                    if matches!(chars.get(j), Some('(' | '[' | '{')) {
                        result.macros_seen += 1;
                        let line = chars[..start].iter().filter(|&&c| c == '\n').count() + 1;
                        let (end, names) = macro_body(&chars, j);
                        for name in names {
                            if FORBIDDEN_NAMES.contains(&name.to_lowercase().as_str()) {
                                result.findings.push(Finding {
                                    line,
                                    macro_name: word.clone(),
                                    name,
                                });
                            }
                        }
                        i = end;
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    result
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("source folder is readable") {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_log_macro_in_src_mentions_transcript_text() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();

    let mut macros_seen = 0;
    let mut violations = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).expect("source file is readable");
        let result = scan(&source);
        macros_seen += result.macros_seen;
        for finding in result.findings {
            violations.push(format!(
                "{}:{}: {}!(...) mentions `{}`",
                file.strip_prefix(&src).unwrap_or(file).display(),
                finding.line,
                finding.macro_name,
                finding.name
            ));
        }
    }

    assert!(
        files.len() >= 8,
        "expected to scan the whole src tree, found {} files",
        files.len()
    );
    assert!(
        macros_seen >= 5,
        "the scanner found only {macros_seen} log macros; it is probably broken"
    );
    assert!(
        violations.is_empty(),
        "log macros must never mention transcript text (SPEC Section 10):\n  {}",
        violations.join("\n  ")
    );
}

// --- The tests below check the scanner itself, using made-up snippets (fixtures, not real code). ---

fn flagged_names(source: &str) -> Vec<String> {
    scan(source).findings.into_iter().map(|f| f.name).collect()
}

#[test]
fn scanner_flags_text_passed_as_an_argument() {
    assert_eq!(
        flagged_names("info!(text = %segment_text);"),
        ["text", "segment_text"]
    );
    assert_eq!(
        flagged_names(r#"warn!("heard", segment.original_text);"#),
        ["original_text"]
    );
    assert_eq!(
        flagged_names(r#"tracing::debug!(transcript = ?t);"#),
        ["transcript"]
    );
    assert_eq!(
        flagged_names(r#"println!("{}", Utterance);"#),
        ["Utterance"]
    );
}

#[test]
fn scanner_flags_text_interpolated_into_the_format_string() {
    assert_eq!(flagged_names(r#"info!("heard {text}");"#), ["text"]);
    assert_eq!(
        flagged_names(r#"error!("failed on {edited_text:?}");"#),
        ["edited_text"]
    );
    assert_eq!(
        flagged_names(r##"info!(r#"raw {caption}"#);"##),
        ["caption"]
    );
}

#[test]
fn scanner_flags_a_call_spread_over_several_lines() {
    let source = "info!(\n    count = 3,\n    text = %t,\n    \"done\"\n);";

    let result = scan(source);

    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].line, 1);
}

#[test]
fn scanner_allows_logging_numbers_ids_and_plain_messages() {
    let source = r#"
        info!(segment_id = 7, chars = 120, "segment finalized");
        warn!(key, "stored setting could not be read; using its default");
        error!(error = %err, "database could not be opened");
        info!("transcription started");
        info!("a literal {{text}} in braces is not a placeholder");
    "#;

    let result = scan(source);

    assert_eq!(result.macros_seen, 5);
    assert!(result.findings.is_empty(), "{:?}", result.findings);
}

#[test]
fn scanner_ignores_comments_and_strings_outside_log_macros() {
    let source = r#"
        // info!(text);
        /* warn!(transcript); */
        let note = "call info!(text) later";
        let other = text;
        let ch = '"';
        fn not_a_macro() { let info = text; }
    "#;

    let result = scan(source);

    assert_eq!(result.macros_seen, 0);
    assert!(result.findings.is_empty(), "{:?}", result.findings);
}

#[test]
fn scanner_is_strict_about_lengths_taken_inline() {
    // Documented behavior: bind the number to a variable first.
    assert_eq!(flagged_names("info!(chars = text.len());"), ["text"]);
    assert!(flagged_names("let chars = text.len(); info!(chars);").is_empty());
}
