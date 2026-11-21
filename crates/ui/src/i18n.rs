//! Interface text: every string a screen shows is a translation key looked up
//! in a table. Only the English table exists for now; French comes later.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// The English table, sorted by key (checked by a test).
const EN: &[(&str, &str)] = &[
    ("overlay.fps", "FPS"),
    ("overlay.frame_avg", "avg"),
    ("overlay.frame_max", "max"),
    ("overlay.frame_ms", "Frame"),
    ("overlay.frame_times", "Frame times"),
    ("overlay.network", "Network"),
    ("overlay.network.in", "In"),
    ("overlay.network.loss", "Loss"),
    ("overlay.network.none", "No connection"),
    ("overlay.network.out", "Out"),
    ("overlay.network.rtt", "Round trip"),
    ("overlay.profiler", "Profiler"),
    ("overlay.profiler.frame", "Last profiled frame"),
    ("overlay.profiler.off", "Profiling scopes are off"),
    ("overlay.simulation", "Simulation"),
    ("overlay.simulation.entities", "Entities"),
    ("overlay.simulation.none", "No simulation"),
    ("overlay.simulation.players", "Players"),
    ("overlay.simulation.tick", "Tick"),
    ("overlay.simulation.tick_ms", "Tick time"),
    ("overlay.title", "Debug"),
    ("overlay.ui", "UI"),
    ("overlay.ui.cpu", "CPU"),
    ("overlay.ui.draw_calls", "Draw calls"),
    ("overlay.ui.vertices", "Vertices"),
];

/// Looks up the interface text of `key`.
///
/// A missing key returns the key itself, so the screen stays usable, and logs
/// a warning once per key.
pub fn text(key: &str) -> &str {
    match EN.binary_search_by_key(&key, |(k, _)| k) {
        Ok(i) => EN[i].1,
        Err(_) => {
            warn_missing(key);
            key
        }
    }
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
    fn table_is_sorted_and_unique() {
        assert!(EN.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn known_key_is_translated() {
        assert_eq!(text("overlay.title"), "Debug");
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
