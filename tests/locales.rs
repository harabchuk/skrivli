//! The translation catalog is complete: every app string exists in every language, every
//! key the code asks for exists, and translations keep the same placeholders.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

const LANGUAGES: [&str; 2] = ["en", "sv"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn catalog() -> Value {
    let text = fs::read_to_string(root().join("locales/ui.json")).expect("read locales/ui.json");
    serde_json::from_str(&text).expect("locales/ui.json is JSON")
}

/// A leaf maps language codes to text. Anything else is a group of keys.
fn is_leaf(map: &serde_json::Map<String, Value>) -> bool {
    !map.is_empty() && map.values().all(Value::is_string)
}

/// Every translation under `node` as (dotted key, language -> text).
fn leaves(prefix: &str, node: &Value, out: &mut Vec<(String, serde_json::Map<String, Value>)>) {
    let Some(map) = node.as_object() else {
        return;
    };
    if is_leaf(map) {
        out.push((prefix.to_string(), map.clone()));
        return;
    }
    for (name, child) in map {
        let key = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        leaves(&key, child, out);
    }
}

fn all_leaves() -> Vec<(String, serde_json::Map<String, Value>)> {
    let mut out = Vec::new();
    leaves("", &catalog(), &mut out);
    out
}

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find("%{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        found.insert(after[..end].to_string());
        rest = &after[end + 1..];
    }
    found
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_app_key_has_every_language() {
    // Arrange
    let leaves = all_leaves();

    // Act
    let missing: Vec<String> = leaves
        .iter()
        .filter(|(key, _)| key.starts_with("app."))
        // Plural forms exist only for the languages that use them.
        .filter(|(key, _)| !key.ends_with("_few") && !key.ends_with("_many"))
        .flat_map(|(key, texts)| {
            LANGUAGES
                .iter()
                .filter(|language| {
                    texts
                        .get(**language)
                        .and_then(Value::as_str)
                        .is_none_or(|text| text.trim().is_empty())
                })
                .map(|language| format!("{key} [{language}]"))
                .collect::<Vec<_>>()
        })
        .collect();

    // Assert
    assert!(missing.is_empty(), "missing translations: {missing:?}");
    assert!(
        leaves.iter().any(|(key, _)| key.starts_with("app.")),
        "no app keys"
    );
}

#[test]
fn translations_keep_the_same_placeholders() {
    // Arrange
    let leaves = all_leaves();

    // Act
    let mismatched: Vec<String> = leaves
        .iter()
        .filter(|(key, _)| key.starts_with("app."))
        .filter(|(_, texts)| {
            let sets: Vec<_> = LANGUAGES
                .iter()
                .filter_map(|language| texts.get(*language).and_then(Value::as_str))
                .map(placeholders)
                .collect();
            sets.windows(2).any(|pair| pair[0] != pair[1])
        })
        .map(|(key, _)| key.clone())
        .collect();

    // Assert
    assert!(mismatched.is_empty(), "placeholders differ: {mismatched:?}");
}

#[test]
fn every_manifest_model_has_a_description_key() {
    // Arrange
    let text = fs::read_to_string(root().join("resources/models/manifest.json")).expect("manifest");
    let manifest: Value = serde_json::from_str(&text).expect("manifest is JSON");
    let keys: BTreeSet<String> = all_leaves().into_iter().map(|(key, _)| key).collect();

    // Act
    let missing: Vec<String> = manifest["models"]
        .as_array()
        .expect("models")
        .iter()
        .map(|model| model["id"].as_str().expect("model id"))
        // Dots and dashes would split or confuse the key, so the id is written with underscores.
        .map(|id| format!("app.model_description.{}", id.replace(['.', '-'], "_")))
        .filter(|key| !keys.contains(key))
        .collect();

    // Assert
    assert!(missing.is_empty(), "no description for {missing:?}");
}

#[test]
fn every_key_the_code_asks_for_exists() {
    // Arrange
    let keys: BTreeSet<String> = all_leaves().into_iter().map(|(key, _)| key).collect();
    let mut files = Vec::new();
    rust_files(&root().join("src"), &mut files);

    // Act
    let mut missing = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).expect("source");
        let mut rest = source.as_str();
        while let Some(start) = rest.find("\"app.") {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else {
                break;
            };
            let key = &after[..end];
            let plain = key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
            if plain && !key.ends_with('.') && !keys.contains(key) {
                missing.push(format!("{} in {}", key, file.display()));
            }
            rest = &after[end + 1..];
        }
    }

    // Assert
    assert!(missing.is_empty(), "unknown keys: {missing:?}");
}

#[test]
fn kit_control_overrides_exist_for_every_language() {
    // Arrange
    let leaves = all_leaves();
    let wanted = [
        "gpui_component.Dialog.ok",
        "gpui_component.Dialog.cancel",
        "gpui_component.Input.Cut",
        "gpui_component.Input.Copy",
        "gpui_component.Input.Paste",
        "gpui_component.Input.Select All",
    ];

    // Act
    let missing: Vec<String> = LANGUAGES
        .iter()
        .filter(|language| **language != "en")
        .flat_map(|language| {
            let leaves = &leaves;
            wanted.iter().filter_map(move |key| {
                let found = leaves
                    .iter()
                    .any(|(name, texts)| name == key && texts.contains_key(*language));
                (!found).then(|| format!("{key} [{language}]"))
            })
        })
        .collect();

    // Assert
    assert!(missing.is_empty(), "no Kit override for {missing:?}");
}
