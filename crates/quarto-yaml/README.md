# quarto-yaml

A YAML 1.2 parser that preserves fine-grained source locations (byte ranges) for
every node in the parsed tree, built on top of
[`yaml-rust2`](https://crates.io/crates/yaml-rust2) and
[`quarto-source-map`](https://crates.io/crates/quarto-source-map).

It produces `YamlWithSourceInfo`, which wraps each `yaml-rust2::Yaml` value with a
`SourceInfo` describing exactly where it came from in the input. This enables
precise, source-located error reporting and lets source provenance survive
transformations such as config merging.

## Example

```rust
use quarto_yaml::parse;

let content = "\
title: My Document
author: John Doe
";

let yaml = parse(content).unwrap();
if let Some(title) = yaml.get_hash_value("title") {
    println!("title starts at byte offset {}", title.source_info.start_offset());
}
```

## Design

Uses an **owned-data** approach: it wraps owned `Yaml` values with a parallel
children structure for source tracking. This trades ~3× memory overhead for
simplicity and for compatibility with merging configs across different lifetimes,
following rust-analyzer's precedent of owned data with reference counting for tree
structures.

## License

MIT — see [LICENSE](../../LICENSE).
