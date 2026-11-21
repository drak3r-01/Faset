use faset_core::glam::{Mat4, Vec3};

use crate::mesh::{Raw, Style};
use crate::prims;
use crate::{Axis, BuildError, Mesh, Palette, Part, Recipe};

const MAX_DEPTH: usize = 12;
const MAX_REPEAT: u32 = 256;

/// Turns a recipe into a mesh. `seed` overrides the recipe's own seed
/// (default 0); the same recipe, palette and seed always give the same mesh.
pub fn build_mesh(
    recipe: &Recipe,
    palette: &Palette,
    seed: Option<u64>,
) -> Result<Mesh, BuildError> {
    let mut b = Builder {
        palette,
        seed: seed.or(recipe.seed).unwrap_or(0),
        stream: 0,
    };
    let mut raw = Raw::default();
    for part in &recipe.parts {
        b.part(part, &mut raw, 0)?;
    }
    tracing::debug!(
        id = %recipe.id,
        vertices = raw.v.len(),
        triangles = raw.i.len() / 3,
        "built mesh"
    );
    Ok(raw.into_mesh())
}

fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic value in [0, 1) from a seed, a stream and a key.
fn unit(seed: u64, stream: u64, k: u64) -> f32 {
    let h = splitmix(splitmix(seed ^ stream.wrapping_mul(0xA24B_AED4_963E_E407)) ^ k);
    (h >> 40) as f32 / (1u64 << 24) as f32
}

struct Builder<'a> {
    palette: &'a Palette,
    seed: u64,
    /// Counts the randomised operations so each one draws its own numbers.
    stream: u64,
}

impl Builder<'_> {
    fn color(&self, name: &str) -> Result<[u8; 3], BuildError> {
        self.palette
            .get(name)
            .ok_or_else(|| BuildError::UnknownColor(name.to_owned()))
    }

    fn style(&self, color: &str, pattern: crate::Pattern) -> Result<Style, BuildError> {
        Ok(Style {
            rgb: self.color(color)?,
            material: pattern as u8,
        })
    }

    fn group(&mut self, parts: &[Part], depth: usize) -> Result<Raw, BuildError> {
        let mut raw = Raw::default();
        for p in parts {
            self.part(p, &mut raw, depth + 1)?;
        }
        Ok(raw)
    }

    fn part(&mut self, part: &Part, out: &mut Raw, depth: usize) -> Result<(), BuildError> {
        if depth > MAX_DEPTH {
            return Err(BuildError::TooDeep(MAX_DEPTH));
        }
        let place = |mut raw: Raw, at: &[f32; 3]| {
            raw.transform(&Mat4::from_translation(Vec3::from_array(*at)));
            raw
        };
        match part {
            Part::Box {
                size,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::cuboid(*size, self.style(color, *pattern)?)?,
                at,
            )),
            Part::Ramp {
                size,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::ramp(*size, self.style(color, *pattern)?)?,
                at,
            )),
            Part::Prism {
                size,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::prism(*size, self.style(color, *pattern)?)?,
                at,
            )),
            Part::Cylinder {
                radius,
                height,
                sides,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::cylinder(*radius, *height, *sides, self.style(color, *pattern)?)?,
                at,
            )),
            Part::Cone {
                radius,
                height,
                sides,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::cone(*radius, *height, *sides, self.style(color, *pattern)?)?,
                at,
            )),
            Part::Sphere {
                radius,
                subdivisions,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::sphere(*radius, *subdivisions, self.style(color, *pattern)?)?,
                at,
            )),
            Part::Torus {
                major_radius,
                minor_radius,
                major_segments,
                minor_segments,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::torus(
                    *major_radius,
                    *minor_radius,
                    *major_segments,
                    *minor_segments,
                    self.style(color, *pattern)?,
                )?,
                at,
            )),
            Part::Extrude {
                polygon,
                height,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::extrude(polygon, *height, self.style(color, *pattern)?)?,
                at,
            )),
            Part::Revolve {
                profile,
                sides,
                at,
                color,
                pattern,
            } => out.append(&place(
                prims::revolve(profile, *sides, self.style(color, *pattern)?)?,
                at,
            )),
            Part::Group {
                at,
                rotate_y,
                scale,
                smooth,
                height_var,
                parts,
            } => {
                let mut scale = Vec3::from_array(*scale);
                if !scale.is_finite() || scale.min_element() <= 0.0 || !height_var.is_finite() {
                    return Err(BuildError::InvalidParameter(
                        "group scale must be finite and > 0".into(),
                    ));
                }
                if *height_var != 0.0 {
                    self.stream += 1;
                    let r = unit(self.seed, self.stream, 0) * 2.0 - 1.0;
                    scale.y *= (1.0 + height_var * r).max(0.05);
                }
                let mut raw = self.group(parts, depth)?;
                if *smooth {
                    raw.smooth_normals();
                }
                raw.transform(
                    &(Mat4::from_translation(Vec3::from_array(*at))
                        * Mat4::from_rotation_y(rotate_y.to_radians())
                        * Mat4::from_scale(scale)),
                );
                out.append(&raw)
            }
            Part::Mirror { axis, parts } => {
                let raw = self.group(parts, depth)?;
                let flip = match axis {
                    Axis::X => Vec3::new(-1.0, 1.0, 1.0),
                    Axis::Y => Vec3::new(1.0, -1.0, 1.0),
                    Axis::Z => Vec3::new(1.0, 1.0, -1.0),
                };
                let mut mirrored = raw.clone();
                mirrored.transform(&Mat4::from_scale(flip));
                out.append(&raw)?;
                out.append(&mirrored)
            }
            Part::Repeat {
                count,
                rotate_y,
                offset,
                part,
            } => {
                if *count > MAX_REPEAT {
                    return Err(BuildError::InvalidParameter(format!(
                        "repeat count must be at most {MAX_REPEAT}"
                    )));
                }
                // Built once per copy so randomised parts differ per copy.
                for i in 0..*count {
                    let mut raw = Raw::default();
                    self.part(part, &mut raw, depth + 1)?;
                    let k = i as f32;
                    raw.transform(
                        &(Mat4::from_translation(Vec3::from_array(*offset) * k)
                            * Mat4::from_rotation_y((rotate_y * k).to_radians())),
                    );
                    out.append(&raw)?;
                }
                Ok(())
            }
            Part::Gradient { from, to, parts } => {
                let (from, to) = (self.color(from)?, self.color(to)?);
                let mut raw = self.group(parts, depth)?;
                let lo = raw.v.iter().map(|v| v.p.y).fold(f32::INFINITY, f32::min);
                let hi = raw
                    .v
                    .iter()
                    .map(|v| v.p.y)
                    .fold(f32::NEG_INFINITY, f32::max);
                let span = (hi - lo).max(1e-6);
                for v in &mut raw.v {
                    let t = ((v.p.y - lo) / span).clamp(0.0, 1.0);
                    for k in 0..3 {
                        let (a, b) = (f32::from(from[k]), f32::from(to[k]));
                        v.c[k] = (a + (b - a) * t).round() as u8;
                    }
                }
                out.append(&raw)
            }
            Part::Jitter { amount, parts } => {
                if !amount.is_finite() || *amount < 0.0 {
                    return Err(BuildError::InvalidParameter(
                        "jitter amount must be >= 0".into(),
                    ));
                }
                self.stream += 1;
                let (seed, stream) = (self.seed, self.stream);
                let mut raw = self.group(parts, depth)?;
                raw.unshare();
                for v in &mut raw.v {
                    let k = Raw::position_key(v.p);
                    let h = (k[0] as u64)
                        .wrapping_mul(73_856_093)
                        .wrapping_add((k[1] as u64).wrapping_mul(19_349_663))
                        .wrapping_add((k[2] as u64).wrapping_mul(83_492_791));
                    let d = Vec3::new(
                        unit(seed, stream, h),
                        unit(seed, stream, h ^ 0x5555),
                        unit(seed, stream, h ^ 0xAAAA),
                    ) * 2.0
                        - Vec3::ONE;
                    v.p += d * *amount;
                }
                raw.recompute_flat_normals();
                out.append(&raw)
            }
        }
    }
}
