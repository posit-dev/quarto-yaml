//! Dump the quarto-yaml 0.4.0 (yaml-rust2 backend) tree for a YAML string:
//! every node's byte span, span text, content-provenance length and tag.
//! Usage: qydump '<yaml source with \n escapes>'

use quarto_yaml::{YamlWithSourceInfo, parse};

fn dump(src: &str, node: &YamlWithSourceInfo, depth: usize) {
    let (s, e) = (node.source_info.start_offset(), node.source_info.end_offset());
    let text = src.get(s..e).map(|t| format!("{t:?}")).unwrap_or_else(|| "<out of bounds>".into());
    let prov = node
        .content_source_info()
        .map(|si| format!("prov={}..{}", si.start_offset(), si.end_offset()))
        .unwrap_or_else(|| if node.is_scalar() { "prov=NONE".into() } else { String::new() });
    let tag = node
        .tag
        .as_ref()
        .map(|(t, si)| format!(" tag={t}@{}..{}", si.start_offset(), si.end_offset()))
        .unwrap_or_default();
    println!("{:indent$}{s}..{e} {text} {:?} {prov}{tag}", "", node.yaml, indent = depth * 2);
    if let Some(items) = node.as_array() {
        for it in items {
            dump(src, it, depth + 1);
        }
    }
    if let Some(entries) = node.as_hash() {
        for en in entries {
            println!(
                "{:indent$}entry {}..{}",
                "",
                en.entry_span.start_offset(),
                en.entry_span.end_offset(),
                indent = depth * 2 + 1
            );
            dump(src, &en.key, depth + 1);
            dump(src, &en.value, depth + 1);
        }
    }
}

fn main() {
    for arg in std::env::args().skip(1) {
        let src = arg.replace("\\n", "\n").replace("\\t", "\t");
        println!("==== {:?}", src);
        match parse(&src) {
            Ok(tree) => dump(&src, &tree, 0),
            Err(e) => println!("ERROR: {e} {:?}", match &e {
                quarto_yaml::Error::ParseError { location, .. } => location.as_ref().map(|l| (l.start_offset(), l.end_offset())),
                _ => None,
            }),
        }
        println!();
    }
}
