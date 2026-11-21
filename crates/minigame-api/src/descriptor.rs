//! Mode descriptors and their typed settings schema.

use std::collections::BTreeMap;

/// Setting key read by [`Match`](crate::Match) as the `Playing` time limit
/// in seconds, when a mode declares it (decision).
pub const TIME_LIMIT_SETTING: &str = "time_limit";

/// Static description of a mode. The host menu is generated from
/// [`settings`](Self::settings).
#[derive(Debug, Clone, PartialEq)]
pub struct ModeDescriptor {
    /// Unique key, such as `race`.
    pub id: &'static str,
    /// Name shown to players.
    pub name: &'static str,
    /// Map labels this mode can run on.
    pub map_labels: &'static [&'static str],
    /// Whether bots can play this mode.
    pub bots: bool,
    /// Settings schema: bounds and defaults.
    pub settings: Vec<SettingSpec>,
}

/// One setting of a mode.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    /// Key, unique within the mode.
    pub key: &'static str,
    /// Label shown in the host menu.
    pub label: &'static str,
    /// Type, bounds and default.
    pub kind: SettingKind,
}

/// Type, bounds and default of a setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingKind {
    /// Integer in `min..=max`.
    Int { min: i32, max: i32, default: i32 },
    /// On or off.
    Bool { default: bool },
    /// Index into `options`.
    Choice {
        options: &'static [&'static str],
        default: usize,
    },
}

impl SettingKind {
    /// The default value.
    #[must_use]
    pub fn default_value(&self) -> SettingValue {
        match *self {
            Self::Int { default, .. } => SettingValue::Int(default),
            Self::Bool { default } => SettingValue::Bool(default),
            Self::Choice { default, .. } => SettingValue::Choice(default),
        }
    }

    fn accepts(&self, value: SettingValue) -> bool {
        match (*self, value) {
            (Self::Int { min, max, .. }, SettingValue::Int(v)) => (min..=max).contains(&v),
            (Self::Bool { .. }, SettingValue::Bool(_)) => true,
            (Self::Choice { options, .. }, SettingValue::Choice(i)) => i < options.len(),
            _ => false,
        }
    }
}

/// A setting value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingValue {
    Int(i32),
    Bool(bool),
    Choice(usize),
}

/// Setting values by key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings(BTreeMap<String, SettingValue>);

impl Settings {
    /// No values: every setting takes its default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets one value.
    pub fn set(&mut self, key: impl Into<String>, value: SettingValue) {
        self.0.insert(key.into(), value);
    }

    /// One value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<SettingValue> {
        self.0.get(key).copied()
    }

    /// An integer value.
    #[must_use]
    pub fn int(&self, key: &str) -> Option<i32> {
        match self.get(key)? {
            SettingValue::Int(v) => Some(v),
            _ => None,
        }
    }

    /// A boolean value.
    #[must_use]
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            SettingValue::Bool(v) => Some(v),
            _ => None,
        }
    }

    /// A choice index.
    #[must_use]
    pub fn choice(&self, key: &str) -> Option<usize> {
        match self.get(key)? {
            SettingValue::Choice(v) => Some(v),
            _ => None,
        }
    }
}

/// A settings value that does not fit the schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// The mode has no setting with this key.
    #[error("unknown setting `{0}`")]
    Unknown(String),
    /// Wrong type or out of bounds.
    #[error("invalid value for setting `{0}`")]
    Invalid(String),
}

impl ModeDescriptor {
    /// Every setting at its default.
    #[must_use]
    pub fn default_settings(&self) -> Settings {
        let mut s = Settings::new();
        for spec in &self.settings {
            s.set(spec.key, spec.kind.default_value());
        }
        s
    }

    /// `values` checked against the schema, missing keys filled with their
    /// defaults.
    ///
    /// # Errors
    /// An unknown key, a wrong type or an out-of-bounds value.
    pub fn resolve(&self, values: &Settings) -> Result<Settings, SettingsError> {
        for (key, &value) in &values.0 {
            let spec = self
                .settings
                .iter()
                .find(|s| s.key == key)
                .ok_or_else(|| SettingsError::Unknown(key.clone()))?;
            if !spec.kind.accepts(value) {
                return Err(SettingsError::Invalid(key.clone()));
            }
        }
        let mut out = self.default_settings();
        out.0.extend(values.0.iter().map(|(k, v)| (k.clone(), *v)));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> ModeDescriptor {
        ModeDescriptor {
            id: "t",
            name: "T",
            map_labels: &[],
            bots: false,
            settings: vec![
                SettingSpec {
                    key: "n",
                    label: "N",
                    kind: SettingKind::Int {
                        min: 1,
                        max: 9,
                        default: 3,
                    },
                },
                SettingSpec {
                    key: "c",
                    label: "C",
                    kind: SettingKind::Choice {
                        options: &["a", "b"],
                        default: 0,
                    },
                },
            ],
        }
    }

    #[test]
    fn resolve_fills_defaults_and_checks_bounds() {
        let d = descriptor();
        let mut v = Settings::new();
        v.set("n", SettingValue::Int(7));
        let r = d.resolve(&v).unwrap();
        assert_eq!(r.int("n"), Some(7));
        assert_eq!(r.choice("c"), Some(0));
        v.set("n", SettingValue::Int(10));
        assert_eq!(d.resolve(&v), Err(SettingsError::Invalid("n".into())));
        v.set("n", SettingValue::Bool(true));
        assert_eq!(d.resolve(&v), Err(SettingsError::Invalid("n".into())));
        let mut u = Settings::new();
        u.set("x", SettingValue::Int(1));
        assert_eq!(d.resolve(&u), Err(SettingsError::Unknown("x".into())));
    }
}
