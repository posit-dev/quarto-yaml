//! Golden-output corpus: every node's span, value, content provenance and
//! tag, for a corpus of YAML inputs, pinned with `insta` snapshots.
//!
//! The point is not to assert that any particular span is right (the unit
//! tests do that) but to make *every* location change visible when the
//! parser or its backend changes: `cargo insta test --review` shows each
//! differing node, to be classified as a fix, neutral, or a regression.
//!
//! Corpus (`tests/corpus/`, exact bytes — see `.gitattributes`):
//! - `edge-cases/`: hand-written inputs that probe marker placement:
//!   non-ASCII in every scalar style, block scalars, empty values, tags,
//!   anchors, flow collections, CRLF, and malformed inputs. Regenerate with
//!   `cargo run --bin gen_corpus` in
//!   `claude-notes/plans/2026-10-08-saphyr-migration-investigation/probe`.
//! - `yaml-test-suite/`: the inputs of the YAML test suite
//!   (<https://github.com/yaml/yaml-test-suite>, MIT, see `LICENSE` there),
//!   decoded from their visual form; `*.fail.yaml` are inputs the suite says
//!   must be rejected. quarto-yaml parses only the first document, so this
//!   is a robustness and accept/reject record, not a conformance test.
//! - the Quarto schema files under
//!   `crates/quarto-yaml-validation/test-fixtures/schemas/`, as real-world
//!   input (block scalars, comments, some non-ASCII).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use quarto_yaml::{Error, SourceInfo, YamlWithSourceInfo, parse};
use yaml_rust2::Yaml;

fn corpus_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name)
}

fn sorted_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .collect();
    files.sort();
    files
}

/// Render a `SourceInfo` as byte offsets. `Original`/`Substring` resolve
/// to `start..end`; a `Concat` (content provenance) lists its pieces as
/// `start..end` when the piece is verbatim (source length == content
/// length) and `start..end=>N` when it is a replacement of N content bytes.
fn render_si(si: &SourceInfo) -> String {
    match si {
        SourceInfo::Concat { pieces } => {
            let parts: Vec<String> = pieces
                .iter()
                .map(|p| {
                    let src = render_si(&p.source_info);
                    if p.source_info.length() == p.length {
                        src
                    } else {
                        format!("{src}=>{}", p.length)
                    }
                })
                .collect();
            format!("[{}]", parts.join(","))
        }
        SourceInfo::Generated(_) => "generated".to_string(),
        _ => match si.resolve_byte_range() {
            Some((_, s, e)) => format!("{s}..{e}"),
            None => "unresolved".to_string(),
        },
    }
}

fn render_yaml(yaml: &Yaml) -> String {
    match yaml {
        Yaml::Array(a) => format!("Array({})", a.len()),
        Yaml::Hash(h) => format!("Hash({})", h.len()),
        other => format!("{other:?}"),
    }
}

fn dump_node(out: &mut String, src: &str, node: &YamlWithSourceInfo, depth: usize) {
    let (s, e) = (
        node.source_info.start_offset(),
        node.source_info.end_offset(),
    );
    let text = src
        .get(s..e)
        .map(|t| format!("{t:?}"))
        .unwrap_or_else(|| "<out of bounds>".to_string());
    let _ = write!(
        out,
        "{:indent$}{s}..{e} {text} {}",
        "",
        render_yaml(&node.yaml),
        indent = depth * 2
    );
    if node.is_scalar() {
        match node.content_source_info() {
            Some(si) => {
                let _ = write!(out, " content={}", render_si(si));
            }
            None => out.push_str(" content=NONE"),
        }
    }
    if let Some((suffix, si)) = &node.tag {
        let _ = write!(out, " tag={suffix}@{}", render_si(si));
    }
    out.push('\n');
    if let Some(items) = node.as_array() {
        for item in items {
            dump_node(out, src, item, depth + 1);
        }
    }
    if let Some(entries) = node.as_hash() {
        for entry in entries {
            let _ = writeln!(
                out,
                "{:indent$}entry {}",
                "",
                render_si(&entry.entry_span),
                indent = depth * 2 + 1
            );
            dump_node(out, src, &entry.key, depth + 1);
            dump_node(out, src, &entry.value, depth + 1);
        }
    }
}

/// Parse `src` and render the tree, the error, or the panic.
fn dump(src: &str) -> String {
    let mut out = String::new();
    match std::panic::catch_unwind(|| parse(src)) {
        Ok(Ok(tree)) => dump_node(&mut out, src, &tree, 0),
        Ok(Err(err)) => {
            let (message, location) = match &err {
                Error::ParseError { message, location }
                | Error::InvalidStructure { message, location } => (message.clone(), location),
                Error::UnexpectedEof { location } => ("unexpected eof".to_string(), location),
            };
            let loc = location
                .as_ref()
                .map(render_si)
                .unwrap_or_else(|| "none".to_string());
            let _ = writeln!(out, "ERROR {message:?} @ {loc}");
        }
        Err(_) => out.push_str("PANIC\n"),
    }
    out
}

fn dump_corpus(files: &[PathBuf]) -> String {
    let mut out = String::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        let bytes = fs::read(path).unwrap();
        let _ = writeln!(out, "==== {name}");
        match std::str::from_utf8(&bytes) {
            Ok(src) => {
                let _ = writeln!(out, "{src:?}");
                out.push_str(&dump(src));
            }
            Err(_) => out.push_str("<not utf-8>\n"),
        }
        out.push('\n');
    }
    out
}

#[test]
fn edge_cases() {
    let files = sorted_files(&corpus_dir("edge-cases"), "yaml");
    assert!(!files.is_empty());
    insta::assert_snapshot!("edge_cases", dump_corpus(&files));
}

#[test]
fn yaml_test_suite() {
    let files = sorted_files(&corpus_dir("yaml-test-suite"), "yaml");
    assert!(!files.is_empty());
    let body = dump_corpus(&files);

    // Summary first, so accept/reject drift is visible at the top.
    let mut ok = 0;
    let mut err = 0;
    let mut panic = 0;
    let mut unexpected = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let must_fail = name.ends_with(".fail.yaml");
        let result = dump(&fs::read_to_string(path).unwrap());
        if result.starts_with("PANIC") {
            panic += 1;
            unexpected.push(format!("{name}: panic"));
        } else if result.starts_with("ERROR") {
            err += 1;
            if !must_fail {
                unexpected.push(format!("{name}: rejected"));
            }
        } else {
            ok += 1;
            if must_fail {
                unexpected.push(format!("{name}: accepted (suite says it must fail)"));
            }
        }
    }
    let mut out = format!(
        "cases: {}  parsed: {ok}  rejected: {err}  panicked: {panic}\n",
        files.len()
    );
    let _ = writeln!(out, "disagreements with the suite ({}):", unexpected.len());
    for line in &unexpected {
        let _ = writeln!(out, "  {line}");
    }
    out.push('\n');
    out.push_str(&body);
    insta::assert_snapshot!("yaml_test_suite", out);
}

#[test]
fn schema_fixtures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../quarto-yaml-validation/test-fixtures/schemas");
    let files = sorted_files(&dir, "yml");
    assert!(!files.is_empty());
    for path in &files {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let src = fs::read_to_string(path).unwrap();
        insta::assert_snapshot!(format!("schema_{stem}"), dump(&src));
    }
}
