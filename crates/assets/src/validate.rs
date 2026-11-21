use crate::{build_mesh, Palette, Recipe};

/// Triangle budget of a full map.
pub const MAP_TRIANGLE_BUDGET: usize = 150_000;

/// Largest extent of one prop, in metres.
const MAX_EXTENT: f32 = 30.0;
/// Smallest useful extent, in metres.
const MIN_EXTENT: f32 = 0.01;

/// Kind of asset, which selects the triangle budget. Taken from the tags
/// `character` and `weapon`; anything else is a prop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Prop,
    Character,
    Weapon,
}

impl AssetKind {
    pub fn of(recipe: &Recipe) -> Self {
        let has = |t: &str| recipe.tags.iter().any(|x| x == t);
        if has("character") {
            Self::Character
        } else if has("weapon") {
            Self::Weapon
        } else {
            Self::Prop
        }
    }

    /// Inclusive triangle budget.
    pub fn budget(self) -> (usize, usize) {
        match self {
            Self::Prop => (20, 200),
            Self::Character => (150, 450),
            Self::Weapon => (200, 600),
        }
    }
}

/// Result of checking one recipe.
#[derive(Debug, Clone, PartialEq)]
pub struct Validation {
    pub id: String,
    pub kind: AssetKind,
    pub triangles: usize,
    pub vertices: usize,
    /// Bounding box size in metres (zero when the build failed).
    pub size: [f32; 3],
    pub errors: Vec<String>,
}

impl Validation {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Checks id, build, determinism, triangle budget and dimensions.
pub fn validate(recipe: &Recipe, palette: &Palette) -> Validation {
    let kind = AssetKind::of(recipe);
    let mut v = Validation {
        id: recipe.id.clone(),
        kind,
        triangles: 0,
        vertices: 0,
        size: [0.0; 3],
        errors: Vec::new(),
    };
    let id_ok = !recipe.id.is_empty()
        && recipe
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !id_ok {
        v.errors
            .push("id must be non-empty lowercase ascii, digits and `_`".into());
    }
    let mesh = match build_mesh(recipe, palette, None) {
        Ok(m) => m,
        Err(e) => {
            v.errors.push(format!("build failed: {e}"));
            return v;
        }
    };
    v.triangles = mesh.triangle_count();
    v.vertices = mesh.vertices.len();
    let (lo, hi) = (kind.budget().0, kind.budget().1);
    if !(lo..=hi).contains(&v.triangles) {
        v.errors.push(format!(
            "{} triangles outside the {kind:?} budget {lo}..={hi}",
            v.triangles
        ));
    }
    if let Some((min, max)) = mesh.bounds() {
        let size = max - min;
        v.size = size.to_array();
        if size
            .to_array()
            .iter()
            .any(|s| !(MIN_EXTENT..=MAX_EXTENT).contains(s))
        {
            v.errors.push(format!(
                "size {:.3} x {:.3} x {:.3} m outside {MIN_EXTENT}..={MAX_EXTENT} m per axis",
                size.x, size.y, size.z
            ));
        }
    }
    if build_mesh(recipe, palette, None).as_ref() != Ok(&mesh) {
        v.errors.push("build is not deterministic".into());
    }
    v
}
