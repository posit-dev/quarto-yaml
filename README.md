# quarto-yaml

A Rust workspace for parsing and validating YAML 1.2 with fine-grained source
locations, extracted from [Quarto](https://github.com/quarto-dev/quarto) so it
can be used standalone.

## Crates

| Crate | Description |
| ----- | ----------- |
| [`quarto-yaml`](crates/quarto-yaml) | A YAML 1.2 parser that preserves byte-range source locations for every node. |
| [`quarto-yaml-validation`](crates/quarto-yaml-validation) | Schema validation for YAML documents, with source-located diagnostics. |

Both crates are published independently to [crates.io](https://crates.io).

## License

MIT — see [LICENSE](LICENSE).
