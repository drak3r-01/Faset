//! Primitive generators. Each returns a mesh centred on its own origin.

use std::f32::consts::TAU;

use faset_core::glam::{Vec2, Vec3};

use crate::mesh::{Raw, Style};
use crate::BuildError;

fn bad(what: &str) -> BuildError {
    BuildError::InvalidParameter(what.to_owned())
}

fn positive(v: f32, what: &str) -> Result<f32, BuildError> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(bad(what))
    }
}

fn size3(size: [f32; 3], what: &str) -> Result<Vec3, BuildError> {
    for c in size {
        positive(c, what)?;
    }
    Ok(Vec3::from_array(size) * 0.5)
}

fn sides(n: u32, what: &str) -> Result<u32, BuildError> {
    if (3..=64).contains(&n) {
        Ok(n)
    } else {
        Err(bad(what))
    }
}

pub(crate) fn cuboid(size: [f32; 3], s: Style) -> Result<Raw, BuildError> {
    let h = size3(size, "box size must be finite and > 0")?;
    let mut r = Raw::default();
    let faces = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    for (n, u, v) in faces {
        let q = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .map(|(su, sv)| h * (n + su * u + sv * v));
        r.quad(q, n, s);
    }
    Ok(r)
}

pub(crate) fn ramp(size: [f32; 3], s: Style) -> Result<Raw, BuildError> {
    let h = size3(size, "ramp size must be finite and > 0")?;
    let p = |x: f32, y: f32, z: f32| Vec3::new(x * h.x, y * h.y, z * h.z);
    // Low edge at +Z (y = -1), high edge at -Z (y = +1).
    let (lb, rb) = (p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0));
    let (lf, rf) = (p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0));
    let (lt, rt) = (p(-1.0, 1.0, -1.0), p(1.0, 1.0, -1.0));
    let mut r = Raw::default();
    r.quad([lf, rf, rb, lb], Vec3::NEG_Y, s);
    r.quad([lf, rf, rt, lt], Vec3::NEG_Z, s);
    r.quad([lt, rt, rb, lb], Vec3::new(0.0, h.z, h.y), s);
    r.tri(lf, lt, lb, Vec3::NEG_X, s);
    r.tri(rf, rt, rb, Vec3::X, s);
    Ok(r)
}

pub(crate) fn prism(size: [f32; 3], s: Style) -> Result<Raw, BuildError> {
    let h = size3(size, "prism size must be finite and > 0")?;
    let (bl, br, top) = (
        Vec3::new(-h.x, -h.y, 0.0),
        Vec3::new(h.x, -h.y, 0.0),
        Vec3::new(0.0, h.y, 0.0),
    );
    let (f, b) = (Vec3::Z * h.z, Vec3::NEG_Z * h.z);
    let mut r = Raw::default();
    r.tri(bl + f, br + f, top + f, Vec3::Z, s);
    r.tri(bl + b, br + b, top + b, Vec3::NEG_Z, s);
    r.quad(
        [bl + b, top + b, top + f, bl + f],
        Vec3::new(-h.y, h.x, 0.0),
        s,
    );
    r.quad(
        [br + b, top + b, top + f, br + f],
        Vec3::new(h.y, h.x, 0.0),
        s,
    );
    r.quad([bl + b, br + b, br + f, bl + f], Vec3::NEG_Y, s);
    Ok(r)
}

fn ring(radius: f32, y: f32, i: u32, n: u32) -> (Vec3, Vec3) {
    let a = TAU * i as f32 / n as f32;
    let dir = Vec3::new(a.cos(), 0.0, a.sin());
    (dir * radius + Vec3::Y * y, dir)
}

pub(crate) fn cylinder(radius: f32, height: f32, n: u32, s: Style) -> Result<Raw, BuildError> {
    positive(radius, "cylinder radius must be > 0")?;
    positive(height, "cylinder height must be > 0")?;
    let n = sides(n, "cylinder sides must be 3 to 64")?;
    let (top, bot) = (height * 0.5, -height * 0.5);
    let mut r = Raw::default();
    for i in 0..n {
        let (a0, d0) = ring(radius, bot, i, n);
        let (a1, d1) = ring(radius, bot, i + 1, n);
        let (b0, _) = ring(radius, top, i, n);
        let (b1, _) = ring(radius, top, i + 1, n);
        r.quad([a0, a1, b1, b0], d0 + d1, s);
        r.tri(Vec3::Y * top, b0, b1, Vec3::Y, s);
        r.tri(Vec3::Y * bot, a0, a1, Vec3::NEG_Y, s);
    }
    Ok(r)
}

pub(crate) fn cone(radius: f32, height: f32, n: u32, s: Style) -> Result<Raw, BuildError> {
    positive(radius, "cone radius must be > 0")?;
    positive(height, "cone height must be > 0")?;
    let n = sides(n, "cone sides must be 3 to 64")?;
    let (top, bot) = (height * 0.5, -height * 0.5);
    let mut r = Raw::default();
    for i in 0..n {
        let (a0, d0) = ring(radius, bot, i, n);
        let (a1, d1) = ring(radius, bot, i + 1, n);
        r.tri(Vec3::Y * top, a0, a1, d0 + d1 + Vec3::Y * 0.01, s);
        r.tri(Vec3::Y * bot, a0, a1, Vec3::NEG_Y, s);
    }
    Ok(r)
}

pub(crate) fn sphere(radius: f32, subdivisions: u32, s: Style) -> Result<Raw, BuildError> {
    positive(radius, "sphere radius must be > 0")?;
    if subdivisions > 3 {
        return Err(bad("sphere subdivisions must be 0 to 3"));
    }
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let base: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let faces: [[usize; 3]; 20] = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    let mut tris: Vec<[Vec3; 3]> = faces
        .iter()
        .map(|f| [base[f[0]], base[f[1]], base[f[2]]])
        .collect();
    for _ in 0..subdivisions {
        tris = tris
            .iter()
            .flat_map(|&[a, b, c]| {
                let (ab, bc, ca) = (
                    (a + b).normalize(),
                    (b + c).normalize(),
                    (c + a).normalize(),
                );
                [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]
            })
            .collect();
    }
    let mut r = Raw::default();
    for [a, b, c] in tris {
        r.tri(a * radius, b * radius, c * radius, a + b + c, s);
    }
    Ok(r)
}

pub(crate) fn torus(
    major: f32,
    minor: f32,
    major_segments: u32,
    minor_segments: u32,
    s: Style,
) -> Result<Raw, BuildError> {
    positive(major, "torus major radius must be > 0")?;
    positive(minor, "torus minor radius must be > 0")?;
    let (m, n) = (
        sides(major_segments, "torus major segments must be 3 to 64")?,
        sides(minor_segments, "torus minor segments must be 3 to 64")?,
    );
    let point = |i: u32, j: u32| {
        let u = TAU * i as f32 / m as f32;
        let v = TAU * j as f32 / n as f32;
        let centre = Vec3::new(u.cos(), 0.0, u.sin()) * major;
        let out = Vec3::new(u.cos() * v.cos(), v.sin(), u.sin() * v.cos());
        (centre + out * minor, out)
    };
    let mut r = Raw::default();
    for i in 0..m {
        for j in 0..n {
            let (p00, n00) = point(i, j);
            let (p10, n10) = point(i + 1, j);
            let (p11, n11) = point(i + 1, j + 1);
            let (p01, n01) = point(i, j + 1);
            r.quad([p00, p10, p11, p01], n00 + n10 + n11 + n01, s);
        }
    }
    Ok(r)
}

fn cross2(o: Vec2, a: Vec2, b: Vec2) -> f32 {
    (a - o).perp_dot(b - o)
}

fn in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let (d1, d2, d3) = (cross2(a, b, p), cross2(b, c, p), cross2(c, a, p));
    d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
}

/// Ear clipping of a simple polygon; returns triangles as index triples.
fn triangulate(poly: &[Vec2]) -> Result<Vec<[usize; 3]>, BuildError> {
    let area: f32 = (0..poly.len())
        .map(|i| poly[i].perp_dot(poly[(i + 1) % poly.len()]))
        .sum();
    if area.abs() < 1e-9 {
        return Err(BuildError::BadPolygon);
    }
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    if area < 0.0 {
        idx.reverse();
    }
    let mut out = Vec::with_capacity(poly.len() - 2);
    while idx.len() > 3 {
        let n = idx.len();
        let ear = (0..n).find(|&k| {
            let (a, b, c) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
            if cross2(poly[a], poly[b], poly[c]) <= 1e-9 {
                return false;
            }
            !idx.iter().any(|&o| {
                o != a && o != b && o != c && in_triangle(poly[o], poly[a], poly[b], poly[c])
            })
        });
        let Some(k) = ear else {
            return Err(BuildError::BadPolygon);
        };
        out.push([idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]]);
        idx.remove(k);
    }
    out.push([idx[0], idx[1], idx[2]]);
    Ok(out)
}

pub(crate) fn extrude(polygon: &[[f32; 2]], height: f32, s: Style) -> Result<Raw, BuildError> {
    positive(height, "extrusion height must be > 0")?;
    if !(3..=64).contains(&polygon.len()) || polygon.iter().flatten().any(|c| !c.is_finite()) {
        return Err(bad("polygon needs 3 to 64 finite points"));
    }
    let pts: Vec<Vec2> = polygon.iter().map(|p| Vec2::from_array(*p)).collect();
    let tris = triangulate(&pts)?;
    let (top, bot) = (height * 0.5, -height * 0.5);
    let at = |p: Vec2, y: f32| Vec3::new(p.x, y, p.y);
    let area: f32 = (0..pts.len())
        .map(|i| pts[i].perp_dot(pts[(i + 1) % pts.len()]))
        .sum();
    let mut r = Raw::default();
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let e = b - a;
        // Counter-clockwise (area > 0) polygons have the outside on the
        // right of each edge.
        let out = Vec2::new(e.y, -e.x) * area.signum();
        r.quad(
            [at(a, bot), at(b, bot), at(b, top), at(a, top)],
            Vec3::new(out.x, 0.0, out.y),
            s,
        );
    }
    for [a, b, c] in tris {
        r.tri(
            at(pts[a], top),
            at(pts[b], top),
            at(pts[c], top),
            Vec3::Y,
            s,
        );
        r.tri(
            at(pts[a], bot),
            at(pts[b], bot),
            at(pts[c], bot),
            Vec3::NEG_Y,
            s,
        );
    }
    Ok(r)
}

pub(crate) fn revolve(profile: &[[f32; 2]], n: u32, s: Style) -> Result<Raw, BuildError> {
    let n = sides(n, "revolve sides must be 3 to 64")?;
    if !(2..=32).contains(&profile.len())
        || profile
            .iter()
            .any(|p| !p[0].is_finite() || !p[1].is_finite() || p[0] < 0.0)
    {
        return Err(bad("profile needs 2 to 32 finite points with radius >= 0"));
    }
    let mut r = Raw::default();
    for w in profile.windows(2) {
        let ([r0, y0], [r1, y1]) = (w[0], w[1]);
        let (dr, dy) = (r1 - r0, y1 - y0);
        for i in 0..n {
            let (p0, d0) = ring(r0, y0, i, n);
            let (p1, d1) = ring(r0, y0, i + 1, n);
            let (q0, _) = ring(r1, y1, i, n);
            let (q1, _) = ring(r1, y1, i + 1, n);
            let mid = (d0 + d1) * dy + Vec3::Y * -dr;
            // Zero-radius ends collapse to a triangle (quad skips if the
            // first triangle is degenerate and uses the second).
            if r0 <= 1e-6 {
                r.tri(p0, q0, q1, mid, s);
            } else if r1 <= 1e-6 {
                r.tri(p0, p1, q0, mid, s);
            } else {
                r.quad([p0, p1, q1, q0], mid, s);
            }
        }
    }
    Ok(r)
}
