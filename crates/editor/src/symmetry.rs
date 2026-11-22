//! Mirror images of map elements for the symmetry modes.
//!
//! Decision: the pivot is the world origin (plane X = 0 for `MirrorX`, plane
//! Z = 0 for `MirrorZ`, point reflection through the origin for `Rotate180`).
//! The transforms are those of [`faset_mapfmt::expand_symmetry`], used
//! through a one-element map, so an element and its twin here are exactly
//! what loading a map that declares the same symmetry would produce.

use faset_mapfmt::{
    expand_symmetry, Brush, Light, Pickup, Placement, Prefab, Prop, Spawn, Symmetry, Zone,
};

use crate::editor::new_map;

/// The twin maker of one symmetry mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mirror(pub Symmetry);

impl Mirror {
    /// Whether the mode produces twins at all.
    #[must_use]
    pub fn is_active(self) -> bool {
        self.0 != Symmetry::None
    }

    fn twin_map(self, fill: impl FnOnce(&mut faset_mapfmt::MapFile)) -> faset_mapfmt::MapFile {
        let mut map = new_map("mirror");
        map.symmetry = self.0;
        fill(&mut map);
        expand_symmetry(&map)
    }

    /// The mirror image of a brush; `None` without symmetry or when the
    /// brush is its own image (centred on the mirror plane).
    #[must_use]
    pub fn brush(self, b: &Brush) -> Option<Brush> {
        if !self.is_active() {
            return None;
        }
        let m = self.twin_map(|m| m.brushes.push(b.clone()));
        m.brushes.into_iter().nth(1).filter(|t| t != b)
    }

    /// The mirror image of a prop.
    #[must_use]
    pub fn prop(self, p: &Prop) -> Option<Prop> {
        if !self.is_active() {
            return None;
        }
        let m = self.twin_map(|m| m.props.push(p.clone()));
        m.props.into_iter().nth(1).filter(|t| t != p)
    }

    /// The mirror image of a spawn (it belongs to the other team).
    #[must_use]
    pub fn spawn(self, s: &Spawn) -> Option<Spawn> {
        if !self.is_active() {
            return None;
        }
        let m = self.twin_map(|m| m.spawns.push(s.clone()));
        m.spawns.into_iter().nth(1).filter(|t| t != s)
    }

    /// The mirror image of a pickup.
    #[must_use]
    pub fn pickup(self, p: &Pickup) -> Option<Pickup> {
        if !self.is_active() {
            return None;
        }
        let m = self.twin_map(|m| m.pickups.push(p.clone()));
        m.pickups.into_iter().nth(1).filter(|t| t != p)
    }

    /// The mirror image of a zone.
    #[must_use]
    pub fn zone(self, z: &Zone) -> Option<Zone> {
        if !self.is_active() {
            return None;
        }
        let m = self.twin_map(|m| m.zones.push(z.clone()));
        m.zones.into_iter().nth(1).filter(|t| t != z)
    }

    /// The mirror image of a light.
    #[must_use]
    pub fn light(self, l: &Light) -> Option<Light> {
        if !self.is_active() {
            return None;
        }
        let m = self.twin_map(|m| m.lights.push(l.clone()));
        m.lights.into_iter().nth(1).filter(|t| t != l)
    }

    /// The mirror image of a prefab definition: same name plus `suffix`,
    /// contents mirrored, no placements.
    #[must_use]
    pub fn prefab_definition(self, p: &Prefab, suffix: &str) -> Option<Prefab> {
        if !self.is_active() {
            return None;
        }
        let m = self.twin_map(|m| {
            m.brushes.clone_from(&p.brushes);
            m.props.clone_from(&p.props);
        });
        let (nb, np) = (p.brushes.len(), p.props.len());
        Some(Prefab {
            name: format!("{}{suffix}", p.name),
            brushes: m.brushes[nb..].to_vec(),
            props: m.props[np..].to_vec(),
            place: Vec::new(),
        })
    }

    /// The mirror image of a placement of `prefab`: the placement of the
    /// mirrored definition, plus whether that definition is the prefab itself
    /// (`Rotate180` is a rotation, so the same definition is reused).
    #[must_use]
    pub fn placement(self, at: &Placement) -> Option<(Placement, bool)> {
        let pos = |p: [i32; 3]| match self.0 {
            Symmetry::None => p,
            Symmetry::MirrorX => [-p[0], p[1], p[2]],
            Symmetry::MirrorZ => [p[0], p[1], -p[2]],
            Symmetry::Rotate180 => [-p[0], p[1], -p[2]],
        };
        match self.0 {
            Symmetry::None => None,
            Symmetry::Rotate180 => Some((
                Placement {
                    at: pos(at.at),
                    turns: (at.turns + 2) % 4,
                },
                true,
            )),
            // A reflection turns quarter turns the other way round.
            Symmetry::MirrorX | Symmetry::MirrorZ => Some((
                Placement {
                    at: pos(at.at),
                    turns: (4 - at.turns % 4) % 4,
                },
                false,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(min: [i32; 3], max: [i32; 3]) -> Brush {
        Brush::Box {
            min,
            max,
            material: "concrete".into(),
        }
    }

    #[test]
    fn mirror_planes_and_pivot() {
        let x = Mirror(Symmetry::MirrorX);
        assert_eq!(
            x.brush(&b([4, 0, 2], [8, 2, 6])),
            Some(b([-8, 0, 2], [-4, 2, 6]))
        );
        let z = Mirror(Symmetry::MirrorZ);
        assert_eq!(
            z.brush(&b([4, 0, 2], [8, 2, 6])),
            Some(b([4, 0, -6], [8, 2, -2]))
        );
        let r = Mirror(Symmetry::Rotate180);
        assert_eq!(
            r.brush(&b([4, 0, 2], [8, 2, 6])),
            Some(b([-8, 0, -6], [-4, 2, -2]))
        );
        assert_eq!(Mirror(Symmetry::None).brush(&b([4, 0, 2], [8, 2, 6])), None);
        // A brush centred on the plane is its own twin.
        assert_eq!(x.brush(&b([-4, 0, 2], [4, 2, 6])), None);
    }
}
