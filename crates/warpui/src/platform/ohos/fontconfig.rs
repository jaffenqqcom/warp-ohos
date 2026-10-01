//! HarmonyOS's own font configuration.
//!
//! HarmonyOS ships no `libfontconfig`; its graphics stack drives font matching
//! from the declarative [`FONTCONFIG_PATH`] instead. The file plays the same
//! role as `/etc/fonts` on Linux: `generic` maps a generic family name (for
//! example `monospace`) onto a concrete one, and `fallback` is an ordered
//! language/script -> family chain. The configuration is read once and cached;
//! when it is missing or malformed the caller falls back to its own heuristic.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Where HarmonyOS keeps its font configuration.
const FONTCONFIG_PATH: &str = "/system/etc/fontconfig.json";

#[derive(Deserialize)]
struct GenericFamily {
    family: String,
    #[serde(default)]
    alias: Vec<HashMap<String, u32>>,
}

#[derive(Deserialize)]
struct FallbackGroup {
    /// The configuration nests the chain under an empty object key. Values that
    /// are not family names (for example `font-variations`) are ignored.
    #[serde(rename = "")]
    entries: Vec<HashMap<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct FontConfigFile {
    #[serde(default)]
    generic: Vec<GenericFamily>,
    #[serde(default)]
    fallback: Vec<FallbackGroup>,
}

/// The parsed configuration, reduced to what font matching needs.
struct FontConfiguration {
    /// Lowercased alias name (generic or postscript) -> concrete family name.
    generic_aliases: HashMap<String, String>,
    /// Fallback family names in configuration order.
    fallback_chain: Vec<String>,
}

/// The device's font configuration, or `None` when it could not be used.
fn configuration() -> Option<&'static FontConfiguration> {
    static CONFIGURATION: OnceLock<Option<FontConfiguration>> = OnceLock::new();

    CONFIGURATION
        .get_or_init(|| {
            let contents = match std::fs::read_to_string(FONTCONFIG_PATH) {
                Ok(contents) => contents,
                Err(error) => {
                    log::warn!("ohos fontconfig: cannot read {FONTCONFIG_PATH}: {error}");
                    return None;
                }
            };
            let parsed: FontConfigFile = match serde_json::from_str(&contents) {
                Ok(parsed) => parsed,
                Err(error) => {
                    log::warn!("ohos fontconfig: cannot parse {FONTCONFIG_PATH}: {error}");
                    return None;
                }
            };

            let mut generic_aliases = HashMap::new();
            for family in &parsed.generic {
                for alias in &family.alias {
                    for name in alias.keys() {
                        generic_aliases.insert(name.to_lowercase(), family.family.clone());
                    }
                }
            }

            let mut fallback_chain: Vec<String> = Vec::new();
            for group in &parsed.fallback {
                for entry in &group.entries {
                    for value in entry.values() {
                        let serde_json::Value::String(family) = value else {
                            continue;
                        };
                        if !fallback_chain.contains(family) {
                            fallback_chain.push(family.clone());
                        }
                    }
                }
            }

            if generic_aliases.is_empty() && fallback_chain.is_empty() {
                log::warn!("ohos fontconfig: {FONTCONFIG_PATH} has no usable entries");
                return None;
            }

            Some(FontConfiguration {
                generic_aliases,
                fallback_chain,
            })
        })
        .as_ref()
}

/// Whether the device's font configuration was loaded successfully.
pub(crate) fn is_loaded() -> bool {
    configuration().is_some()
}

/// Resolves a generic or postscript font name to its configured family.
pub(crate) fn generic_alias_of(name: &str) -> Option<&'static str> {
    configuration()?
        .generic_aliases
        .get(&name.to_lowercase())
        .map(String::as_str)
}

/// The configured fallback family names, in order.
pub(crate) fn fallback_family_chain() -> &'static [String] {
    configuration()
        .map(|configuration| configuration.fallback_chain.as_slice())
        .unwrap_or_default()
}
