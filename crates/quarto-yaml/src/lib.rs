//! # quarto-yaml
//!
//! YAML parsing with source location tracking.
//!
//! This crate provides `YamlWithSourceInfo`, which wraps a [`Yaml`] value
//! (yaml-rust2's value type) with source location information for every node
//! in the YAML tree. This enables precise error reporting and source tracking
//! through transformations. Parsing is done with `saphyr-parser`; the
//! `yaml-rust2` dependency supplies only the value type.
//!
//! ## Design
//!
//! Uses the **owned data approach**: wraps owned `Yaml` values with a parallel
//! children structure for source tracking. Trade-off: ~3x memory overhead for
//! simplicity and compatibility with config merging across different lifetimes.
//!
//! Follows rust-analyzer's precedent of using owned data with reference counting
//! for tree structures.
//!
//! ## Example
//!
//! ```rust,no_run
//! use quarto_yaml::parse;
//!
//! let content = r#"
//! title: My Document
//! author: John Doe
//! "#;
//!
//! let yaml = parse(content).unwrap();
//! // Access with source location tracking
//! if let Some(title) = yaml.get_hash_value("title") {
//!     println!("Title at offset {}", title.source_info.start_offset());
//! }
//! ```

#[cfg(test)]
mod content_provenance_tests;
mod error;
mod parser;
mod yaml_with_source_info;

pub use error::{Error, Result};
pub use parser::{file_id_for_filename, parse, parse_file, parse_with_parent};
pub use quarto_source_map::SourceInfo; // Re-export from quarto-source-map
pub use yaml_with_source_info::{YamlHashEntry, YamlWithSourceInfo};

/// The value type held in [`YamlWithSourceInfo::yaml`].
///
/// Currently `yaml_rust2::Yaml`, re-exported so consumers can name it
/// without depending on `yaml-rust2` themselves. Prefer these paths: a
/// future release will replace the re-export with a type defined in this
/// crate, and code that names `yaml_rust2` directly will have to change then.
pub use yaml_rust2::Yaml;

/// [`Yaml`] together with its collection types (`Array`, `Hash`).
pub mod yaml {
    pub use yaml_rust2::yaml::{Array, Hash, Yaml};
}
