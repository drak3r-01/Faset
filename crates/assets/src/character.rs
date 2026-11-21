//! Rigid-block characters: a character recipe split into six limbs that are
//! moved by [`Pose`]s, with no mesh deformation.
//!
//! Recipe convention: a recipe tagged `character` has exactly six top-level
//! [`Part::Group`]s, in the order of [`Limb`]. The group's `at` is the joint
//! pivot (the hip for the torso, the neck for the head, the shoulders and
//! hips for the limbs); its children are drawn relative to that pivot. The
//! characters face -Z.

use faset_core::glam::{Mat4, Vec3};

use crate::anim::Pose;
use crate::{build_mesh, BuildError, Mesh, Palette, Part, Recipe};

/// The six rigid blocks of a character, in recipe order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limb {
    Head = 0,
    Torso = 1,
    ArmL = 2,
    ArmR = 3,
    LegL = 4,
    LegR = 5,
}

impl Limb {
    pub const ALL: [Limb; 6] = [
        Limb::Head,
        Limb::Torso,
        Limb::ArmL,
        Limb::ArmR,
        Limb::LegL,
        Limb::LegR,
    ];
}

/// Why a recipe is not a valid character.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RigError {
    #[error("a character needs six top-level groups (head, torso, arms, legs), found {0} parts")]
    PartCount(usize),
    #[error("character part {0} is not a Group")]
    NotAGroup(usize),
    #[error(transparent)]
    Build(#[from] BuildError),
}

/// One rigid block, built around its pivot.
#[derive(Debug, Clone, PartialEq)]
pub struct RigPart {
    pub limb: Limb,
    /// Joint position in the rest pose.
    pub pivot: Vec3,
    /// Mesh with the pivot at its origin.
    pub mesh: Mesh,
}

/// A character split into its rigid blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct Rig {
    pub parts: [RigPart; 6],
}

impl Rig {
    /// Builds the six blocks of a character recipe.
    pub fn from_recipe(recipe: &Recipe, palette: &Palette) -> Result<Self, RigError> {
        if recipe.parts.len() != 6 {
            return Err(RigError::PartCount(recipe.parts.len()));
        }
        let mut parts = Vec::with_capacity(6);
        for (i, part) in recipe.parts.iter().enumerate() {
            let Part::Group {
                at,
                smooth,
                height_var,
                parts: children,
                ..
            } = part
            else {
                return Err(RigError::NotAGroup(i));
            };
            let local = Recipe {
                id: recipe.id.clone(),
                tags: Vec::new(),
                collision: recipe.collision,
                seed: recipe.seed,
                parts: vec![Part::Group {
                    at: [0.0; 3],
                    rotate_y: 0.0,
                    scale: [1.0; 3],
                    smooth: *smooth,
                    height_var: *height_var,
                    parts: children.clone(),
                }],
            };
            parts.push(RigPart {
                limb: Limb::ALL[i],
                pivot: Vec3::from_array(*at),
                mesh: build_mesh(&local, palette, None)?,
            });
        }
        Ok(Self {
            parts: parts.try_into().expect("six parts"),
        })
    }

    /// World-from-local matrix of each block (indexed by [`Limb`]) for a pose,
    /// relative to the character's feet. Head, torso and arms lean about the
    /// hip; the whole body bobs.
    pub fn part_transforms(&self, pose: &Pose) -> [Mat4; 6] {
        let hip = self.parts[Limb::Torso as usize].pivot;
        let about = |pivot: Vec3, angle: f32| {
            Mat4::from_translation(pivot)
                * Mat4::from_rotation_x(angle)
                * Mat4::from_translation(-pivot)
        };
        let body = Mat4::from_translation(Vec3::new(0.0, pose.bob, 0.0));
        // Forward lean pitches the upper body towards -Z (negative about X).
        let upper = body * about(hip, -pose.lean);
        let angle = |limb: Limb| match limb {
            Limb::ArmL => pose.arm_l,
            Limb::ArmR => pose.arm_r,
            Limb::LegL => pose.leg_l,
            Limb::LegR => pose.leg_r,
            _ => 0.0,
        };
        std::array::from_fn(|i| {
            let p = &self.parts[i];
            let base = match p.limb {
                Limb::LegL | Limb::LegR => body,
                _ => upper,
            };
            base * Mat4::from_translation(p.pivot) * Mat4::from_rotation_x(angle(p.limb))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::limb_pose;

    fn block(at: [f32; 3]) -> Part {
        Part::Group {
            at,
            rotate_y: 0.0,
            scale: [1.0; 3],
            smooth: false,
            height_var: 0.0,
            parts: vec![Part::Box {
                size: [0.2, 0.4, 0.2],
                at: [0.0, -0.2, 0.0],
                color: "metal".into(),
                pattern: Default::default(),
            }],
        }
    }

    fn recipe() -> Recipe {
        Recipe {
            id: "test".into(),
            tags: vec!["character".into()],
            collision: Default::default(),
            seed: None,
            parts: vec![
                block([0.0, 1.6, 0.0]),
                block([0.0, 0.9, 0.0]),
                block([-0.3, 1.5, 0.0]),
                block([0.3, 1.5, 0.0]),
                block([-0.1, 0.9, 0.0]),
                block([0.1, 0.9, 0.0]),
            ],
        }
    }

    #[test]
    fn pivots_and_local_meshes() {
        let rig = Rig::from_recipe(&recipe(), &Palette::default()).unwrap();
        assert_eq!(
            rig.parts[Limb::ArmR as usize].pivot,
            Vec3::new(0.3, 1.5, 0.0)
        );
        let (lo, hi) = rig.parts[2].mesh.bounds().unwrap();
        assert!(
            lo.y < -0.39 && hi.y.abs() < 1e-6,
            "mesh hangs below its pivot"
        );
    }

    #[test]
    fn neutral_pose_is_the_rest_pose() {
        let rig = Rig::from_recipe(&recipe(), &Palette::default()).unwrap();
        for (i, m) in rig.part_transforms(&Pose::NEUTRAL).iter().enumerate() {
            let at = m.transform_point3(Vec3::ZERO);
            assert!((at - rig.parts[i].pivot).length() < 1e-6);
        }
    }

    #[test]
    fn a_forward_swing_moves_the_foot_forward() {
        let rig = Rig::from_recipe(&recipe(), &Palette::default()).unwrap();
        let pose = Pose {
            leg_l: 0.8,
            ..Pose::NEUTRAL
        };
        let m = rig.part_transforms(&pose)[Limb::LegL as usize];
        let foot = m.transform_point3(Vec3::new(0.0, -0.4, 0.0));
        assert!(foot.z < -0.2, "{foot}");
        let moving = rig.part_transforms(&limb_pose(5.0, 1.0));
        assert_ne!(moving, rig.part_transforms(&Pose::NEUTRAL));
    }

    #[test]
    fn wrong_layouts_are_rejected() {
        let mut r = recipe();
        r.parts.pop();
        assert_eq!(
            Rig::from_recipe(&r, &Palette::default()).unwrap_err(),
            RigError::PartCount(5)
        );
        let mut r = recipe();
        r.parts[0] = Part::Box {
            size: [1.0; 3],
            at: [0.0; 3],
            color: "metal".into(),
            pattern: Default::default(),
        };
        assert_eq!(
            Rig::from_recipe(&r, &Palette::default()).unwrap_err(),
            RigError::NotAGroup(0)
        );
    }
}
