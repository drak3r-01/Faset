//! Interface translations: every UI string is a key in a table.
//!
//! The English table (`lang/en.toml`) is complete; the French one
//! (`lang/fr.toml`) must have every key of it (checked by a test). Lookup
//! order: the current language, then English, then the key itself.

use std::collections::HashMap;

use crate::settings::Language;

const EN_TEXT: &str = include_str!("../lang/en.toml");
const FR_TEXT: &str = include_str!("../lang/fr.toml");

/// A table that could not be read.
#[derive(Debug, thiserror::Error)]
pub enum TranslationError {
    /// The text is not valid TOML.
    #[error("cannot parse the {0} table: {1}")]
    Parse(&'static str, toml::de::Error),
    /// A value is not a string.
    #[error("key `{0}` of the {1} table is not a string")]
    NotAString(String, &'static str),
}

/// The English and French tables and the current language.
#[derive(Debug, Clone)]
pub struct Translations {
    language: Language,
    en: HashMap<String, String>,
    fr: HashMap<String, String>,
}

fn parse_table(
    text: &str,
    name: &'static str,
) -> Result<HashMap<String, String>, TranslationError> {
    let table: toml::Table = text.parse().map_err(|e| TranslationError::Parse(name, e))?;
    table
        .into_iter()
        .map(|(k, v)| match v {
            toml::Value::String(s) => Ok((k, s)),
            _ => Err(TranslationError::NotAString(k, name)),
        })
        .collect()
}

impl Translations {
    /// The tables shipped with the game, in English.
    ///
    /// # Panics
    /// If the embedded tables are malformed (a test guards against it).
    pub fn builtin() -> Self {
        Self::from_texts(EN_TEXT, FR_TEXT).expect("the embedded translation tables are valid")
    }

    /// Reads the two tables from TOML text (flat `"key" = "text"` pairs).
    ///
    /// # Errors
    /// If a table is not valid TOML or holds a value that is not a string.
    pub fn from_texts(en: &str, fr: &str) -> Result<Self, TranslationError> {
        Ok(Self {
            language: Language::English,
            en: parse_table(en, "English")?,
            fr: parse_table(fr, "French")?,
        })
    }

    /// The language [`Self::t`] answers in.
    pub fn language(&self) -> Language {
        self.language
    }

    /// Switches language; the next lookup uses it.
    pub fn set_language(&mut self, language: Language) {
        self.language = language;
    }

    /// The text of `key` in `language`, if the table of that language, or
    /// English as a fallback, has it.
    pub fn get_in(&self, language: Language, key: &str) -> Option<&str> {
        let table = match language {
            Language::English => &self.en,
            Language::French => &self.fr,
        };
        table
            .get(key)
            .or_else(|| self.en.get(key))
            .map(String::as_str)
    }

    /// The text of `key` in the current language; a key missing from French
    /// shows its English text, one missing from both shows the key.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.get_in(self.language, key).unwrap_or(key)
    }

    /// [`Self::t`] in an explicit language.
    pub fn t_in<'a>(&'a self, language: Language, key: &'a str) -> &'a str {
        self.get_in(language, key).unwrap_or(key)
    }

    /// Every English key, sorted.
    pub fn keys(&self) -> Vec<&str> {
        let mut keys: Vec<_> = self.en.keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    /// English keys the French table lacks, sorted.
    pub fn missing_in_french(&self) -> Vec<&str> {
        let mut keys: Vec<_> = self
            .en
            .keys()
            .filter(|k| !self.fr.contains_key(*k))
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    /// French keys the English table lacks (leftovers), sorted.
    pub fn extra_in_french(&self) -> Vec<&str> {
        let mut keys: Vec<_> = self
            .fr
            .keys()
            .filter(|k| !self.en.contains_key(*k))
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_has_every_english_key() {
        let t = Translations::builtin();
        assert_eq!(t.missing_in_french(), Vec::<&str>::new());
        assert_eq!(t.extra_in_french(), Vec::<&str>::new());
        assert!(t.keys().len() > 100);
    }

    #[test]
    fn switching_language_changes_the_strings_at_the_next_lookup() {
        let mut t = Translations::builtin();
        let keys: Vec<String> = t.keys().iter().map(|k| (*k).to_owned()).collect();
        let english: Vec<String> = keys.iter().map(|k| t.t(k).to_owned()).collect();
        t.set_language(Language::French);
        let french: Vec<String> = keys.iter().map(|k| t.t(k).to_owned()).collect();
        // Most strings differ; words that are the same in both languages
        // (`FASET`, `FPS`, ...) may stay equal.
        let changed = english.iter().zip(&french).filter(|(e, f)| e != f).count();
        assert!(changed * 10 > keys.len() * 7, "{changed} of {}", keys.len());
        assert_eq!(t.t("menu.play"), t.t_in(Language::French, "menu.play"));
        t.set_language(Language::English);
        let again: Vec<String> = keys.iter().map(|k| t.t(k).to_owned()).collect();
        assert_eq!(again, english);
    }

    #[test]
    fn missing_keys_fall_back_as_specified() {
        let t = Translations::from_texts("a = \"one\"\nb = \"two\"\n", "a = \"un\"\n").unwrap();
        let mut t = t;
        t.set_language(Language::French);
        assert_eq!(t.t("a"), "un");
        // Missing in French: English.
        assert_eq!(t.t("b"), "two");
        // Missing in both: the key.
        assert_eq!(t.t("c.d"), "c.d");
        t.set_language(Language::English);
        assert_eq!(t.t("c.d"), "c.d");
    }

    #[test]
    fn bad_tables_are_reported() {
        assert!(matches!(
            Translations::from_texts("a = ", ""),
            Err(TranslationError::Parse("English", _))
        ));
        assert!(matches!(
            Translations::from_texts("", "a = 3"),
            Err(TranslationError::NotAString(_, "French"))
        ));
    }
}
