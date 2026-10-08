//! Regenerate `crates/quarto-yaml/tests/corpus/` from the edge-case list and
//! from the yaml-test-suite checkout inside the saphyr clone.
//!
//! Usage (from this directory): `cargo run --bin gen_corpus`
//!
//! Edge cases are written as `edge-cases/NN-<slug>.yaml` with exact bytes
//! (no newline normalisation; see `.gitattributes`). yaml-test-suite cases
//! are decoded from their visual form the same way saphyr's runner does
//! (`parser/tests/yaml-test-suite.rs::visual_to_raw`) and written as
//! `yaml-test-suite/<ID>[-NN].yaml`, with `fail: true` cases included
//! (quarto-yaml should reject them too). Cases marked `skip` are omitted.

#[path = "../cases.rs"]
mod cases;

use std::fs;
use std::path::Path;

use saphyr::{LoadableYamlNode, Yaml};

fn visual_to_raw(yaml: &str) -> String {
    let mut yaml = yaml.to_owned();
    for (pat, replacement) in [
        ("␣", " "),
        ("»", "\t"),
        ("—", ""),
        ("←", "\r"),
        ("⇔", "\u{FEFF}"),
        ("↵", ""),
        ("∎\n", ""),
    ] {
        yaml = yaml.replace(pat, replacement);
    }
    yaml
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn main() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = here.join("../../../..").canonicalize().unwrap();
    let corpus = repo.join("crates/quarto-yaml/tests/corpus");
    let suite_src = repo.join("saphyr/parser/tests/yaml-test-suite/src");

    // --- edge cases -------------------------------------------------------
    let dir = corpus.join("edge-cases");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (i, (name, src)) in cases::cases().iter().enumerate() {
        let path = dir.join(format!("{i:02}-{}.yaml", slug(name)));
        fs::write(&path, src.as_bytes()).unwrap();
    }
    println!("wrote {} edge cases to {}", cases::cases().len(), dir.display());

    // --- yaml-test-suite --------------------------------------------------
    if !suite_src.is_dir() {
        eprintln!("no yaml-test-suite at {}; skipping", suite_src.display());
        return;
    }
    let dir = corpus.join("yaml-test-suite");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::copy(
        suite_src.join("../License"),
        dir.join("LICENSE"),
    )
    .unwrap();
    let mut entries: Vec<_> = fs::read_dir(&suite_src)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .collect();
    entries.sort();
    let mut n = 0;
    for path in entries {
        let id = path.file_stem().unwrap().to_string_lossy().to_string();
        let text = fs::read_to_string(&path).unwrap();
        let docs = Yaml::load_from_str(&text).unwrap();
        let tests = docs[0].as_vec().expect("test list");
        // Fields other than `fail` are inherited from the previous test in the file.
        let mut current = saphyr::Mapping::new();
        for (idx, t) in tests.iter().enumerate() {
            current.remove(&Yaml::value_from_str("fail"));
            for (k, v) in t.as_mapping().unwrap().clone() {
                current.insert(k, v);
            }
            let cur = Yaml::Mapping(current.clone());
            if cur.contains_mapping_key("skip") {
                continue;
            }
            let name = if tests.len() > 1 {
                format!("{id}-{idx:02}")
            } else {
                id.clone()
            };
            let fail = cur
                .as_mapping_get("fail")
                .and_then(|f| f.as_bool())
                .unwrap_or(false);
            let yaml = visual_to_raw(cur["yaml"].as_str().unwrap());
            let file = if fail {
                format!("{name}.fail.yaml")
            } else {
                format!("{name}.yaml")
            };
            fs::write(dir.join(file), yaml.as_bytes()).unwrap();
            n += 1;
        }
    }
    println!("wrote {n} yaml-test-suite cases to {}", dir.display());
}
