//! Architecture test: keeps the layers' dependency rule true as the code changes.
//!
//! ```text
//! presentation ──► application ──► domain
//!                       ▲
//! infrastructure ───────┘
//! ```
//!
//! Each layer is a folder under `src/`. A layer may only name the layers it's allowed to
//! depend on, and the inner layers may not use UI, HTTP, or Windows crates at all.
//! Only `main.rs` (the composition root) sees everything.

use std::fs;
use std::path::{Path, PathBuf};

/// (layer, other layers it must not reference)
const LAYER_RULES: &[(&str, &[&str])] = &[
    ("domain", &["application", "infrastructure", "presentation"]),
    ("application", &["infrastructure", "presentation"]),
    ("infrastructure", &["presentation"]),
    ("presentation", &["infrastructure"]),
];

/// External crates that belong in the outer layers only.
const OUTER_CRATES: &[&str] = &[
    "tao", "wry", "tray_icon", "softbuffer", "tiny_skia", "ab_glyph", "ureq", "keyring", "windows_sys", "windows_registry",
    "serde_json", "toml",
];
const INNER_LAYERS: &[&str] = &["domain", "application"];

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|x| x == "rs") {
            files.push(path);
        }
    }
    files
}

/// Code lines only, so doc comments may still mention other layers.
fn code_lines(path: &Path) -> Vec<(usize, String)> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .map(|(i, l)| (i + 1, l.to_string()))
        .collect()
}

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn layers_only_depend_inward() {
    let mut violations = Vec::new();
    for (layer, forbidden) in LAYER_RULES {
        for file in rust_files(&src().join(layer)) {
            for (n, line) in code_lines(&file) {
                for other in *forbidden {
                    if line.contains(&format!("crate::{other}")) {
                        violations.push(format!("{}:{n}: {layer} must not use {other}: {}", file.display(), line.trim()));
                    }
                }
            }
        }
    }
    assert!(violations.is_empty(), "dependency rule broken:\n{}", violations.join("\n"));
}

#[test]
fn inner_layers_use_no_ui_http_or_os_crates() {
    let mut violations = Vec::new();
    for layer in INNER_LAYERS {
        for file in rust_files(&src().join(layer)) {
            for (n, line) in code_lines(&file) {
                for krate in OUTER_CRATES {
                    if line.contains(&format!("{krate}::")) {
                        violations.push(format!("{}:{n}: {layer} must not use `{krate}`: {}", file.display(), line.trim()));
                    }
                }
            }
        }
    }
    assert!(violations.is_empty(), "inner layer reaches outside:\n{}", violations.join("\n"));
}

#[test]
fn every_layer_folder_exists() {
    for (layer, _) in LAYER_RULES {
        assert!(src().join(layer).join("mod.rs").is_file(), "missing src/{layer}/mod.rs");
    }
}
