# quarto-yaml

A YAML 1.2 parser that preserves fine-grained source locations (byte ranges) for
every node in the parsed tree, built on top of
[`saphyr`](https://crates.io/crates/saphyr) and
[`quarto-source-map`](https://crates.io/crates/quarto-source-map).

It produces `YamlWithSourceInfo`, which wraps each `saphyr::YamlOwned` value with a
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

## Scalar resolution

Scalars are typed by the YAML 1.2 core schema, so quoting means what the spec
says it means:

- A quoted or block scalar is always a string: `"1"` is the *string* `"1"`,
  not an integer, and `""` is the empty string, not null.
- An explicit standard tag decides the type, overriding both quoting and
  implicit resolution: `!!str 5` is a string, `!!int "7"` is an integer. A value
  that contradicts its tag (`!!int abc`) resolves to `YamlOwned::BadValue`.
- Only `true`/`false` (and their `True`, `TRUE` spellings) are booleans. YAML
  1.1's `yes`, `no`, `on` and `off` are plain strings in 1.2.
- Application-specific tags (`!expr`, `!path`, …) don't affect resolution; they
  are reported in `YamlWithSourceInfo::tag`, alongside the tag's own span, for
  consumers to interpret.

Spans are measured against the source rather than the decoded value, so a
quoted scalar's span covers its quotes and escapes, and a multi-line plain or
block scalar's span covers every line it occupies.

## Design

Uses an **owned-data** approach: it wraps owned `Yaml` values with a parallel
children structure for source tracking. This trades ~3× memory overhead for
simplicity and for compatibility with merging configs across different lifetimes,
following rust-analyzer's precedent of owned data with reference counting for tree
structures.

## License

MIT — see [LICENSE](../../LICENSE).
