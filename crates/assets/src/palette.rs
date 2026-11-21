use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Named linear colours of a map theme (`wood`, `metal`, `accent1`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Palette {
    colors: BTreeMap<String, [u8; 3]>,
}

impl Palette {
    /// Builds a palette from `(name, linear rgb)` pairs.
    pub fn new(colors: impl IntoIterator<Item = (String, [u8; 3])>) -> Self {
        Self {
            colors: colors.into_iter().collect(),
        }
    }

    /// Parses a RON map such as `{"wood": (150, 100, 60)}`.
    pub fn from_ron(text: &str) -> Result<Self, crate::RecipeError> {
        Ok(ron::from_str(text)?)
    }

    pub fn get(&self, name: &str) -> Option<[u8; 3]> {
        self.colors.get(name).copied()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.colors.keys().map(String::as_str)
    }

    /// The neutral theme used when a map ships none.
    pub fn default_theme() -> Self {
        let table: [(&str, [u8; 3]); 16] = [
            ("wood", [150, 100, 60]),
            ("wood_dark", [90, 58, 34]),
            ("metal", [150, 156, 164]),
            ("metal_dark", [70, 74, 82]),
            ("stone", [128, 124, 116]),
            ("concrete", [165, 165, 160]),
            ("brick", [158, 82, 66]),
            ("rubber", [34, 34, 38]),
            ("foliage", [60, 130, 60]),
            ("foliage_dark", [36, 88, 44]),
            ("sand", [212, 190, 140]),
            ("white", [235, 235, 235]),
            ("accent1", [230, 120, 40]),
            ("accent2", [50, 130, 220]),
            ("accent3", [220, 60, 90]),
            ("accent4", [240, 210, 60]),
        ];
        Self::new(table.map(|(n, c)| (n.to_owned(), c)))
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::default_theme()
    }
}
