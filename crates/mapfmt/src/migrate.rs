//! Format migration: `format_version` selects a chain of `vN -> vN+1` steps.
//!
//! A step rewrites the RON text of version N into version N+1 (including the
//! `format_version` field); the last step's output is read with the current
//! schema. To add version N+1: freeze what is needed of the vN schema inside
//! the step, append the step to [`MIGRATIONS`] and bump
//! [`crate::FORMAT_VERSION`].

use serde::Deserialize;

use crate::{MapError, MapFile, FORMAT_VERSION};

/// Rewrites a file of version N (its index + 1) into version N+1.
pub type Migration = fn(&str) -> Result<String, MapError>;

/// `MIGRATIONS[i]` migrates version `i + 1` to `i + 2`.
const MIGRATIONS: [Migration; (FORMAT_VERSION - 1) as usize] = [v1_to_v2, v2_to_v3];

/// Version 2 only adds the optional `terrain` field: a version 1 file is the
/// same file with its version number rewritten.
fn v1_to_v2(text: &str) -> Result<String, MapError> {
    set_version(text, 2)
}

/// Version 3 only adds the optional `vegetation` field: a version 2 file is
/// the same file with its version number rewritten.
fn v2_to_v3(text: &str) -> Result<String, MapError> {
    set_version(text, 3)
}

/// Rewrites the number after the first `format_version:` key.
fn set_version(text: &str, version: u32) -> Result<String, MapError> {
    let missing = || MapError::Invalid("no format_version field to migrate".to_owned());
    let key = text.find("format_version").ok_or_else(missing)?;
    let after = key + "format_version".len();
    let rest = &text[after..];
    let colon = rest
        .find(|c: char| !c.is_whitespace())
        .filter(|&i| rest[i..].starts_with(':'))
        .ok_or_else(missing)?;
    let value = &rest[colon + 1..];
    let start = value
        .find(|c: char| !c.is_whitespace())
        .ok_or_else(missing)?;
    let len = value[start..]
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len() - start);
    let at = after + colon + 1 + start;
    Ok(format!("{}{version}{}", &text[..at], &text[at + len..]))
}

/// Reads only the version, ignoring every other field.
#[derive(Deserialize)]
#[serde(rename = "Map")]
struct VersionProbe {
    format_version: u32,
}

/// Peeks the version of `text`, runs the steps needed and parses the result.
pub fn migrate(text: &str) -> Result<MapFile, MapError> {
    let found = ron::from_str::<VersionProbe>(text)?.format_version;
    let text = run_chain(text, found, &MIGRATIONS)?;
    let map: MapFile = ron::from_str(&text)?;
    if map.format_version == FORMAT_VERSION {
        Ok(map)
    } else {
        Err(MapError::Invalid(format!(
            "migration ended at version {}, expected {FORMAT_VERSION}",
            map.format_version
        )))
    }
}

/// Applies `chain[found - 1..]` to `text`. Versions are 1-based; the newest
/// readable version is `chain.len() + 1`.
fn run_chain(text: &str, found: u32, chain: &[Migration]) -> Result<String, MapError> {
    let newest = chain.len() + 1;
    let start = usize::try_from(found).unwrap_or(usize::MAX);
    if start == 0 || start > newest {
        return Err(MapError::UnknownVersion { found });
    }
    let mut text = text.to_owned();
    for step in &chain[start - 1..] {
        text = step(&text)?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1_to_v2(t: &str) -> Result<String, MapError> {
        Ok(t.replace("format_version: 1", "format_version: 2") + "//2")
    }

    fn v2_to_v3(t: &str) -> Result<String, MapError> {
        Ok(t.replace("format_version: 2", "format_version: 3") + "//3")
    }

    #[test]
    fn version_one_is_rewritten() {
        assert_eq!(
            super::v1_to_v2("Map(\n    format_version :  1,\n)").unwrap(),
            "Map(\n    format_version :  2,\n)"
        );
        assert_eq!(
            super::v1_to_v2("Map(format_version:1)").unwrap(),
            "Map(format_version:2)"
        );
        let map = migrate("Map(format_version: 1, meta: (name: \"m\", modes: [], players: (1, 2)), theme: (ambience: \"day\", palette: \"x\"))").unwrap();
        assert_eq!(map.format_version, FORMAT_VERSION);
        assert!(map.terrain.is_none());
        assert!(map.vegetation.is_none());
        assert_eq!(
            super::v2_to_v3("Map(format_version: 2)").unwrap(),
            "Map(format_version: 3)"
        );
    }

    #[test]
    fn chain_runs_from_found_version() {
        let chain: [Migration; 2] = [v1_to_v2, v2_to_v3];
        assert_eq!(
            run_chain("format_version: 1", 1, &chain).unwrap(),
            "format_version: 3//2//3"
        );
        assert_eq!(
            run_chain("format_version: 2", 2, &chain).unwrap(),
            "format_version: 3//3"
        );
        assert_eq!(run_chain("x", 3, &chain).unwrap(), "x");
        assert!(matches!(
            run_chain("x", 4, &chain),
            Err(MapError::UnknownVersion { found: 4 })
        ));
        assert!(matches!(
            run_chain("x", 0, &chain),
            Err(MapError::UnknownVersion { found: 0 })
        ));
    }
}
