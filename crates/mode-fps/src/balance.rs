//! Time-to-kill maths and the table printed by `cargo xtask balance` (lot 4.6).
//!
//! `TTK = (ceil(PV / D) - 1) * 60 / rpm`: the first shot lands at t = 0, so
//! only the gaps between shots count. D is the body damage of one trigger
//! pull at point-blank range (no falloff): `pellets * base` for hitscan and
//! melee, and `pellets * explosion damage` for a projectile with a blast
//! (the direct hit of the plan's area weapons). Decision: the head-shot TTK
//! uses `base * head_multiplier` and is reported for single-pellet hitscan
//! weapons only.

use std::fmt::Write as _;

use crate::class::{Catalog, ClassDef};
use crate::weapon::{WeaponDef, WeaponKind};

/// Lower edge of the target band for the starting pairs, seconds.
pub const TTK_MIN: f64 = 0.4;
/// Upper edge of the target band, seconds.
pub const TTK_MAX: f64 = 0.8;

/// Tolerance so that exact band edges (0.4 s) count as inside.
const EPS: f64 = 1e-6;

/// Output format of [`report`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Csv,
}

/// Damage of one trigger pull on a body, at point-blank range.
#[must_use]
pub fn trigger_damage(w: &WeaponDef) -> f64 {
    let blast = w.projectile.as_ref().and_then(|p| p.explosion.as_ref());
    let per_pellet = match (w.kind, blast) {
        (WeaponKind::Projectile, Some(e)) => e.damage,
        _ => w.damage.base,
    };
    f64::from(per_pellet) * f64::from(w.pellets)
}

/// Seconds to deal `health` with `damage` per pull at `rpm`; `None` when the
/// weapon deals no damage.
#[must_use]
pub fn ttk(health: f64, damage: f64, rpm: f64) -> Option<f64> {
    if damage <= 0.0 || rpm <= 0.0 {
        return None;
    }
    let shots = (health / damage - 1e-9).ceil().max(1.0);
    Some((shots - 1.0) * 60.0 / rpm)
}

/// Body TTK of a weapon against a class.
#[must_use]
pub fn body_ttk(w: &WeaponDef, c: &ClassDef) -> Option<f64> {
    ttk(f64::from(c.health), trigger_damage(w), f64::from(w.rpm))
}

/// Head-shot TTK, for single-pellet hitscan weapons with a head bonus.
#[must_use]
pub fn head_ttk(w: &WeaponDef, c: &ClassDef) -> Option<f64> {
    if w.kind != WeaponKind::Hitscan || w.pellets != 1 || w.damage.head_multiplier <= 1.0 {
        return None;
    }
    let d = f64::from(w.damage.base) * f64::from(w.damage.head_multiplier);
    ttk(f64::from(c.health), d, f64::from(w.rpm))
}

/// Whether a TTK sits inside the target band.
#[must_use]
pub fn in_band(t: f64) -> bool {
    (TTK_MIN - EPS..=TTK_MAX + EPS).contains(&t)
}

/// The weapon x class matrix: one row per weapon, one cell per class.
#[must_use]
pub fn matrix(catalog: &Catalog) -> Vec<(&WeaponDef, Vec<Option<f64>>)> {
    catalog
        .weapons()
        .map(|w| {
            let row = catalog.classes().iter().map(|c| body_ttk(w, c)).collect();
            (w, row)
        })
        .collect()
}

/// `(class, main weapon, ttk)` triples outside the band.
#[must_use]
pub fn main_deviations(catalog: &Catalog) -> Vec<(String, String, f64)> {
    let mut out = Vec::new();
    for c in catalog.classes() {
        if let Some(w) = catalog.weapon(&c.main_weapon) {
            match body_ttk(w, c) {
                Some(t) if in_band(t) => {}
                Some(t) => out.push((c.id.clone(), w.id.clone(), t)),
                None => out.push((c.id.clone(), w.id.clone(), f64::INFINITY)),
            }
        }
    }
    out
}

fn cell(t: Option<f64>) -> String {
    match t {
        None => "-".to_owned(),
        Some(t) if in_band(t) => format!("{t:.3}"),
        Some(t) => format!("{t:.3}*"),
    }
}

/// Renders the report. Text: the matrix, then the main-weapon table with
/// head-shot TTK; CSV: one `weapon,class,ttk_s,head_ttk_s,in_band` line per pair.
#[must_use]
pub fn report(catalog: &Catalog, format: Format) -> String {
    let mut out = String::new();
    match format {
        Format::Csv => {
            out.push_str("weapon,class,ttk_s,head_ttk_s,in_band\n");
            for w in catalog.weapons() {
                for c in catalog.classes() {
                    let t = body_ttk(w, c);
                    let _ = writeln!(
                        out,
                        "{},{},{},{},{}",
                        w.id,
                        c.id,
                        t.map_or(String::new(), |t| format!("{t:.3}")),
                        head_ttk(w, c).map_or(String::new(), |t| format!("{t:.3}")),
                        t.is_some_and(in_band),
                    );
                }
            }
        }
        Format::Text => {
            let _ = writeln!(
                out,
                "TTK in seconds, target {TTK_MIN} to {TTK_MAX} s ('*' = outside, '-' = no damage)\n"
            );
            let _ = write!(out, "{:<26}", "weapon \\ class");
            for c in catalog.classes() {
                let _ = write!(out, "{:>10}", c.id);
            }
            out.push('\n');
            let _ = write!(out, "{:<26}", "(health)");
            for c in catalog.classes() {
                let _ = write!(out, "{:>10}", c.health);
            }
            out.push('\n');
            for (w, row) in matrix(catalog) {
                let _ = write!(out, "{:<26}", w.id);
                for t in row {
                    let _ = write!(out, "{:>10}", cell(t));
                }
                out.push('\n');
            }
            out.push_str("\nMain weapons\n");
            let _ = writeln!(
                out,
                "{:<10}{:<26}{:>8}{:>10}{:>10}",
                "class", "weapon", "dmg", "body", "head"
            );
            for c in catalog.classes() {
                let Some(w) = catalog.weapon(&c.main_weapon) else {
                    continue;
                };
                let _ = writeln!(
                    out,
                    "{:<10}{:<26}{:>8}{:>10}{:>10}",
                    c.id,
                    w.id,
                    trigger_damage(w),
                    cell(body_ttk(w, c)),
                    head_ttk(w, c).map_or("-".to_owned(), |t| format!("{t:.3}")),
                );
            }
            let dev = main_deviations(catalog);
            if dev.is_empty() {
                out.push_str("\nAll main-weapon pairs are inside the band.\n");
            } else {
                for (c, w, t) in dev {
                    let _ = writeln!(out, "\nOUTSIDE: {c} with {w}: {t:.3} s");
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trooper_ttk_is_four_tenths() {
        let c = Catalog::builtin();
        let t = body_ttk(
            c.weapon("assault_rifle").unwrap(),
            c.class("trooper").unwrap(),
        )
        .unwrap();
        assert!((t - 0.4).abs() < 1e-9, "{t}");
        // The plan's own check values.
        assert!((ttk(100.0, 24.0, 600.0).unwrap() - 0.4).abs() < 1e-9);
        assert!((ttk(85.0, 17.0, 900.0).unwrap() - 4.0 * 60.0 / 900.0).abs() < 1e-9);
        assert_eq!(ttk(100.0, 0.0, 600.0), None);
    }

    #[test]
    fn every_main_weapon_pair_is_inside_the_band() {
        let c = Catalog::builtin();
        assert!(main_deviations(&c).is_empty(), "{:?}", main_deviations(&c));
    }

    #[test]
    fn marksman_headshot_is_fatal() {
        let c = Catalog::builtin();
        let (w, m) = (
            c.weapon("precision_rifle").unwrap(),
            c.class("marksman").unwrap(),
        );
        assert_eq!(head_ttk(w, m), Some(0.0));
        assert!(head_ttk(c.weapon("pump_shotgun").unwrap(), m).is_none());
    }

    #[test]
    fn table_covers_all_six_classes_and_every_weapon() {
        let c = Catalog::builtin();
        let text = report(&c, Format::Text);
        for class in c.classes() {
            assert!(text.contains(&class.id));
        }
        for w in c.weapons() {
            assert!(text.contains(&w.id));
        }
        let csv = report(&c, Format::Csv);
        assert_eq!(csv.lines().count(), 1 + 12 * 6);
        assert!(matrix(&c).iter().all(|(_, row)| row.len() == 6));
    }
}
