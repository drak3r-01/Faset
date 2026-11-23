//! Interface text: every string a screen shows is a translation key looked up
//! in the tables of `faset_core::translations` (English and French).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use faset_core::settings::Language;
use faset_core::translations::Translations;

/// Looks up the interface text of `key` in the current language.
///
/// A key missing from French shows its English text; one missing from both
/// returns the key itself, so the screen stays usable, and logs a warning
/// once per key.
pub fn text(key: &str) -> &str {
    match try_text(key) {
        Some(t) => t,
        None => {
            warn_missing(key);
            key
        }
    }
}

/// Like [`text`] without the fallback to the key (and without the warning).
pub fn try_text(key: &str) -> Option<&'static str> {
    tables().get_in(language(), key)
}

/// Replaces each `{name}` of `template` with its value.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.to_owned();
    for (name, value) in values {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

fn tables() -> &'static Translations {
    static TABLES: OnceLock<Translations> = OnceLock::new();
    TABLES.get_or_init(Translations::builtin)
}

static LANGUAGE: AtomicU8 = AtomicU8::new(0);

/// The language every screen is drawn in.
pub fn language() -> Language {
    Language::from_index(usize::from(LANGUAGE.load(Ordering::Relaxed))).unwrap_or(Language::English)
}

/// Switches the language; the next frame draws in it.
pub fn set_language(language: Language) {
    LANGUAGE.store(language.index() as u8, Ordering::Relaxed);
}

fn warn_missing(key: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.insert(key.to_owned()) {
        tracing::warn!(key, "missing translation key");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_key_is_translated_and_language_switches() {
        // The global language is only switched in `tests/language.rs`
        // (its own process), so these unit tests stay deterministic.
        assert_eq!(language(), Language::English);
        assert_eq!(text("overlay.title"), "Debug");
        assert_eq!(fill("a {x} b", &[("x", "1")]), "a 1 b");
    }

    #[test]
    fn missing_key_returns_itself_and_logs_a_warning() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tracing_subscriber::layer::SubscriberExt;

        struct Warnings(Arc<AtomicUsize>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Warnings {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                if *event.metadata().level() == tracing::Level::WARN {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        let count = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry().with(Warnings(count.clone()));
        let _guard = tracing::subscriber::set_default(subscriber);
        assert_eq!(text("no.such.key"), "no.such.key");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // Warned once per key.
        assert_eq!(text("no.such.key"), "no.such.key");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
