use faset_assets::{
    build_mesh, validate, Axis, BuildError, Collision, Mesh, Palette, Part, Pattern, Recipe,
};
use faset_core::glam::Vec3;
use proptest::prelude::*;

fn recipe(parts: Vec<Part>) -> Recipe {
    Recipe {
        id: "t".into(),
        tags: vec![],
        collision: Collision::Box,
        seed: None,
        parts,
    }
}

fn build(parts: Vec<Part>) -> Mesh {
    build_mesh(&recipe(parts), &Palette::default(), None).unwrap()
}

fn assert_counts(m: &Mesh, vertices: usize, triangles: usize) {
    assert_eq!(m.vertices.len(), vertices, "vertices");
    assert_eq!(m.triangle_count(), triangles, "triangles");
    assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
}

fn wood() -> String {
    "wood".into()
}

macro_rules! prim {
    ($v:ident { $($f:tt)* }) => {
        Part::$v { $($f)*, at: [0.0; 3], color: wood(), pattern: Pattern::Flat }
    };
}

#[test]
fn primitive_counts_match_parameters() {
    assert_counts(&build(vec![prim!(Box { size: [1.0; 3] })]), 24, 12);
    assert_counts(&build(vec![prim!(Ramp { size: [1.0; 3] })]), 18, 8);
    assert_counts(&build(vec![prim!(Prism { size: [1.0; 3] })]), 18, 8);
    for n in [3u32, 8, 16] {
        let k = n as usize;
        let cyl = prim!(Cylinder {
            radius: 0.5,
            height: 1.0,
            sides: n
        });
        assert_counts(&build(vec![cyl]), 10 * k, 4 * k);
        let cone = prim!(Cone {
            radius: 0.5,
            height: 1.0,
            sides: n
        });
        assert_counts(&build(vec![cone]), 6 * k, 2 * k);
    }
    for s in 0..=3u32 {
        let tris = 20 * 4usize.pow(s);
        let sphere = prim!(Sphere {
            radius: 1.0,
            subdivisions: s
        });
        assert_counts(&build(vec![sphere]), 3 * tris, tris);
    }
    let torus = prim!(Torus {
        major_radius: 1.0,
        minor_radius: 0.25,
        major_segments: 12,
        minor_segments: 6
    });
    assert_counts(&build(vec![torus]), 4 * 72, 2 * 72);
    // L-shaped polygon: 6 edges, 4 triangles per cap.
    let l_shape = prim!(Extrude {
        polygon: vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0]
        ],
        height: 1.0
    });
    assert_counts(&build(vec![l_shape]), 4 * 6 + 2 * 4 * 3, 2 * 6 + 2 * 4);
    let vase = prim!(Revolve {
        profile: vec![[0.2, 0.0], [0.4, 0.5], [0.3, 1.0]],
        sides: 8
    });
    assert_counts(&build(vec![vase]), 2 * 8 * 4, 2 * 8 * 2);
}

#[test]
fn flat_normals_are_unit_and_point_outward() {
    let parts = vec![
        Part::Box {
            size: [1.0, 2.0, 3.0],
            at: [0.0; 3],
            color: wood(),
            pattern: Pattern::Flat,
        },
        Part::Sphere {
            radius: 1.0,
            subdivisions: 2,
            at: [5.0, 0.0, 0.0],
            color: wood(),
            pattern: Pattern::Flat,
        },
        Part::Cylinder {
            radius: 1.0,
            height: 2.0,
            sides: 12,
            at: [-5.0, 0.0, 0.0],
            color: wood(),
            pattern: Pattern::Flat,
        },
        Part::Torus {
            major_radius: 1.0,
            minor_radius: 0.3,
            major_segments: 12,
            minor_segments: 6,
            at: [0.0, 0.0, 6.0],
            color: wood(),
            pattern: Pattern::Flat,
        },
    ];
    let m = build(parts);
    for t in m.indices.chunks(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| &m.vertices[i as usize]);
        let (pa, pb, pc) = (
            Vec3::from_array(a.position),
            Vec3::from_array(b.position),
            Vec3::from_array(c.position),
        );
        let n = a.normal_vec();
        assert!(
            (n.length() - 1.0).abs() < 0.02,
            "normal length {}",
            n.length()
        );
        // Counter-clockwise winding agrees with the stored normal.
        assert!((pb - pa).cross(pc - pa).dot(n) > 0.0);
    }
    // A convex solid's faces point away from its centre.
    let sphere = build(vec![Part::Sphere {
        radius: 1.0,
        subdivisions: 1,
        at: [0.0; 3],
        color: wood(),
        pattern: Pattern::Flat,
    }]);
    for v in &sphere.vertices {
        assert!(Vec3::from_array(v.position).dot(v.normal_vec()) > 0.0);
    }
}

#[test]
fn extrude_normals_point_outward_for_either_winding() {
    for flip in [false, true] {
        let mut poly = vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        if flip {
            poly.reverse();
        }
        let m = build(vec![Part::Extrude {
            polygon: poly,
            height: 2.0,
            at: [0.0; 3],
            color: wood(),
            pattern: Pattern::Flat,
        }]);
        for v in &m.vertices {
            assert!(Vec3::from_array(v.position).dot(v.normal_vec()) > 0.0);
        }
    }
}

fn varied() -> Recipe {
    recipe(vec![Part::Group {
        at: [0.0; 3],
        rotate_y: 0.0,
        scale: [1.0; 3],
        smooth: false,
        height_var: 0.4,
        parts: vec![
            Part::Jitter {
                amount: 0.2,
                parts: vec![Part::Sphere {
                    radius: 1.0,
                    subdivisions: 1,
                    at: [0.0; 3],
                    color: wood(),
                    pattern: Pattern::Noise,
                }],
            },
            Part::Repeat {
                count: 3,
                rotate_y: 120.0,
                offset: [0.0; 3],
                part: Box::new(Part::Jitter {
                    amount: 0.1,
                    parts: vec![Part::Box {
                        size: [0.3; 3],
                        at: [1.0, 0.0, 0.0],
                        color: "metal".into(),
                        pattern: Pattern::Flat,
                    }],
                }),
            },
        ],
    }])
}

#[test]
fn same_seed_gives_identical_mesh_and_other_seeds_differ() {
    let (r, p) = (varied(), Palette::default());
    let a = build_mesh(&r, &p, Some(7)).unwrap();
    assert_eq!(a, build_mesh(&r, &p, Some(7)).unwrap());
    assert_ne!(a, build_mesh(&r, &p, Some(8)).unwrap());
    let mut seeded = r.clone();
    seeded.seed = Some(7);
    assert_eq!(a, build_mesh(&seeded, &p, None).unwrap());
}

#[test]
fn operations_transform_mirror_repeat_gradient_smooth() {
    let boxed = |at: [f32; 3]| Part::Box {
        size: [1.0; 3],
        at,
        color: wood(),
        pattern: Pattern::Flat,
    };
    // Mirror doubles the mesh and keeps faces outward.
    let m = build(vec![Part::Mirror {
        axis: Axis::X,
        parts: vec![boxed([2.0, 0.0, 0.0])],
    }]);
    assert_counts(&m, 48, 24);
    let (lo, hi) = m.bounds().unwrap();
    assert!((lo.x + 2.5).abs() < 1e-4 && (hi.x - 2.5).abs() < 1e-4);
    for v in &m.vertices {
        let centre = Vec3::new(v.position[0].signum() * 2.0, 0.0, 0.0);
        assert!((Vec3::from_array(v.position) - centre).dot(v.normal_vec()) > 0.0);
    }
    // Repeat turns the part about Y (the spec example: four posts).
    let m = build(vec![Part::Repeat {
        count: 4,
        rotate_y: 90.0,
        offset: [0.0; 3],
        part: Box::new(boxed([1.0, 0.0, 0.0])),
    }]);
    assert_counts(&m, 96, 48);
    // Group scales, turns and moves.
    let m = build(vec![Part::Group {
        at: [0.0, 5.0, 0.0],
        rotate_y: 0.0,
        scale: [2.0, 1.0, 1.0],
        smooth: false,
        height_var: 0.0,
        parts: vec![boxed([0.0; 3])],
    }]);
    let (lo, hi) = m.bounds().unwrap();
    assert!((hi - lo - Vec3::new(2.0, 1.0, 1.0)).length() < 1e-4);
    assert!((lo.y - 4.5).abs() < 1e-4);
    // Gradient colours the lowest vertices `from` and the highest `to`.
    let p = Palette::default();
    let m = build(vec![Part::Gradient {
        from: "white".into(),
        to: "rubber".into(),
        parts: vec![boxed([0.0; 3])],
    }]);
    let (low, high) = (
        m.vertices.iter().find(|v| v.position[1] < 0.0).unwrap(),
        m.vertices.iter().find(|v| v.position[1] > 0.0).unwrap(),
    );
    assert_eq!(low.color_material[..3], p.get("white").unwrap());
    assert_eq!(high.color_material[..3], p.get("rubber").unwrap());
    // A smoothing group keeps the vertex count and averages normals.
    let m = build(vec![Part::Group {
        at: [0.0; 3],
        rotate_y: 0.0,
        scale: [1.0; 3],
        smooth: true,
        height_var: 0.0,
        parts: vec![boxed([0.0; 3])],
    }]);
    assert_eq!(m.vertices.len(), 24);
    for v in &m.vertices {
        let n = v.normal_vec();
        assert!((n.x.abs() - n.y.abs()).abs() < 0.02 && (n.y.abs() - n.z.abs()).abs() < 0.02);
    }
}

#[test]
fn pattern_sets_material_id() {
    let m = build(vec![Part::Box {
        size: [1.0; 3],
        at: [0.0; 3],
        color: wood(),
        pattern: Pattern::Planks,
    }]);
    assert!(m.vertices.iter().all(|v| v.color_material[3] == 7));
}

#[test]
fn errors_are_reported() {
    let b = |p: Vec<Part>| build_mesh(&recipe(p), &Palette::default(), None);
    assert_eq!(
        b(vec![Part::Box {
            size: [1.0; 3],
            at: [0.0; 3],
            color: "nope".into(),
            pattern: Pattern::Flat
        }]),
        Err(BuildError::UnknownColor("nope".into()))
    );
    assert!(matches!(
        b(vec![Part::Cylinder {
            radius: 1.0,
            height: 1.0,
            sides: 2,
            at: [0.0; 3],
            color: wood(),
            pattern: Pattern::Flat
        }]),
        Err(BuildError::InvalidParameter(_))
    ));
    assert_eq!(
        b(vec![Part::Extrude {
            polygon: vec![[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]],
            height: 1.0,
            at: [0.0; 3],
            color: wood(),
            pattern: Pattern::Flat
        }]),
        Err(BuildError::BadPolygon)
    );
}

#[test]
fn spec_example_parses_and_validates() {
    let text = r#"
Prop(
    id: "crate_wood",
    tags: ["cover", "urban"],
    collision: Box,
    parts: [
        Box(size: (1.0, 1.0, 1.0), color: "wood", pattern: Planks),
        Repeat(count: 4, rotate_y: 90.0, part:
            Box(size: (0.08, 1.02, 0.08), at: (0.5, 0.0, 0.5), color: "wood_dark")),
    ],
)
"#;
    let r = Recipe::from_ron(text).unwrap();
    assert_eq!(r.id, "crate_wood");
    let v = validate(&r, &Palette::default());
    // 5 boxes = 60 triangles, within the prop budget.
    assert_eq!(v.triangles, 60);
    assert!(v.is_ok(), "{:?}", v.errors);
}

#[test]
fn validator_flags_budget_and_unknown_colour() {
    let heavy = recipe(vec![Part::Sphere {
        radius: 1.0,
        subdivisions: 3,
        at: [0.0; 3],
        color: wood(),
        pattern: Pattern::Flat,
    }]);
    assert!(!validate(&heavy, &Palette::default()).is_ok());
    let light = recipe(vec![Part::Box {
        size: [1.0; 3],
        at: [0.0; 3],
        color: wood(),
        pattern: Pattern::Flat,
    }]);
    assert!(!validate(&light, &Palette::default()).is_ok(), "12 < 20");
}

fn f() -> impl Strategy<Value = f32> {
    -100.0f32..100.0
}

fn f3() -> impl Strategy<Value = [f32; 3]> {
    [f(), f(), f()]
}

fn pattern() -> impl Strategy<Value = Pattern> {
    prop::sample::select(vec![
        Pattern::Flat,
        Pattern::Noise,
        Pattern::Planks,
        Pattern::Brick,
        Pattern::Hologram,
    ])
}

fn leaf() -> impl Strategy<Value = Part> {
    prop_oneof![
        (f3(), f3(), "[a-z_]{1,8}", pattern()).prop_map(|(size, at, color, pattern)| Part::Box {
            size,
            at,
            color,
            pattern
        }),
        (f(), f(), 3u32..64, f3(), "[a-z_]{1,8}", pattern()).prop_map(
            |(radius, height, sides, at, color, pattern)| Part::Cylinder {
                radius,
                height,
                sides,
                at,
                color,
                pattern
            }
        ),
        (
            prop::collection::vec([f(), f()], 3..8),
            f(),
            f3(),
            "[a-z]{1,8}"
        )
            .prop_map(|(polygon, height, at, color)| Part::Extrude {
                polygon,
                height,
                at,
                color,
                pattern: Pattern::Flat
            }),
    ]
}

fn part() -> impl Strategy<Value = Part> {
    leaf().prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            (
                f3(),
                f(),
                f3(),
                any::<bool>(),
                f(),
                prop::collection::vec(inner.clone(), 0..3)
            )
                .prop_map(|(at, rotate_y, scale, smooth, height_var, parts)| {
                    Part::Group {
                        at,
                        rotate_y,
                        scale,
                        smooth,
                        height_var,
                        parts,
                    }
                }),
            (0u32..8, f(), f3(), inner.clone()).prop_map(|(count, rotate_y, offset, p)| {
                Part::Repeat {
                    count,
                    rotate_y,
                    offset,
                    part: Box::new(p),
                }
            }),
            (
                prop::sample::select(vec![Axis::X, Axis::Y, Axis::Z]),
                prop::collection::vec(inner.clone(), 0..3)
            )
                .prop_map(|(axis, parts)| Part::Mirror { axis, parts }),
            (f(), prop::collection::vec(inner, 0..3))
                .prop_map(|(amount, parts)| Part::Jitter { amount, parts }),
        ]
    })
}

proptest! {
    #[test]
    fn recipe_ron_round_trip_is_stable(
        id in "[a-z_]{1,12}",
        tags in prop::collection::vec("[a-z]{1,6}", 0..3),
        seed in prop::option::of(any::<u64>()),
        collision in prop::sample::select(vec![
            Collision::None, Collision::Box, Collision::ConvexHull, Collision::Compound,
        ]),
        parts in prop::collection::vec(part(), 0..4),
    ) {
        let r = Recipe { id, tags, collision, seed, parts };
        let text = r.to_ron().unwrap();
        let back = Recipe::from_ron(&text).unwrap();
        prop_assert_eq!(&back, &r);
        prop_assert_eq!(back.to_ron().unwrap(), text);
    }
}
