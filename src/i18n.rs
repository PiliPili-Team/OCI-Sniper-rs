use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};

static I18N: OnceLock<I18nCatalog> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct I18nCatalog {
    default_locale: String,
    translations: HashMap<String, HashMap<String, String>>,
}

impl I18nCatalog {
    pub fn load_from_dir(dir: &Path, default_locale: impl Into<String>) -> Result<Self> {
        let mut translations = HashMap::new();
        for locale in ["en", "zh-CN", "zh-TW"] {
            let path = dir.join(format!("{locale}.json"));
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read locale file: {}", path.display()))?;
            let entries = serde_json::from_str::<HashMap<String, String>>(&raw)
                .with_context(|| format!("failed to parse locale file: {}", path.display()))?;
            translations.insert(locale.to_string(), entries);
        }

        Ok(Self {
            default_locale: default_locale.into(),
            translations,
        })
    }

    pub fn initialize(dir: &Path, default_locale: impl Into<String>) -> Result<&'static Self> {
        if let Some(existing) = I18N.get() {
            return Ok(existing);
        }

        let catalog = Self::load_from_dir(dir, default_locale)?;
        let _ = I18N.set(catalog);
        Ok(I18N.get().expect("i18n catalog must be initialized"))
    }

    pub fn global() -> &'static Self {
        I18N.get()
            .expect("i18n catalog must be initialized before use")
    }

    pub fn t(&self, locale: &str, key: &str, replacements: &[(&str, &str)]) -> String {
        self.lookup(locale, key)
            .or_else(|| self.lookup(&self.default_locale, key))
            .map(|value| interpolate(value, replacements))
            .unwrap_or_else(|| {
                eprintln!(
                    "warn: missing i18n key '{key}' for locale '{locale}', falling back to raw key"
                );
                key.to_string()
            })
    }

    fn lookup(&self, locale: &str, key: &str) -> Option<&str> {
        self.translations
            .get(locale)
            .and_then(|locale_map| locale_map.get(key))
            .map(String::as_str)
    }
}

pub fn locales_dir() -> PathBuf {
    let local = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("locales");
    if local.exists() {
        return local;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("locales")
}

fn interpolate(template: &str, replacements: &[(&str, &str)]) -> String {
    replacements
        .iter()
        .fold(template.to_string(), |acc, (key, value)| {
            acc.replace(&format!("{{{key}}}"), value)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_english_when_key_missing_in_requested_locale() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("en.json"), r#"{"hello":"Hello {name}"}"#).unwrap();
        fs::write(dir.path().join("zh-CN.json"), r#"{}"#).unwrap();
        fs::write(dir.path().join("zh-TW.json"), r#"{}"#).unwrap();

        let catalog = I18nCatalog::load_from_dir(dir.path(), "en").unwrap();
        let value = catalog.t("zh-CN", "hello", &[("name", "Alice")]);

        assert_eq!(value, "Hello Alice");
    }
}
