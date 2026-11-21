/// Why a recipe file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    #[error("invalid RON: {0}")]
    Parse(#[from] ron::error::SpannedError),
    #[error("cannot write RON: {0}")]
    Write(#[from] ron::Error),
}

/// Why a recipe could not be turned into a mesh.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BuildError {
    #[error("unknown palette colour `{0}`")]
    UnknownColor(String),
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),
    #[error("polygon is degenerate or self-intersecting")]
    BadPolygon,
    #[error("recipe nests deeper than {0} levels")]
    TooDeep(usize),
    #[error("mesh exceeds {0} vertices")]
    TooLarge(usize),
}
