//! Minimal include preprocessor for WGSL.
//!
//! A line whose trimmed text is `#include "name.wgsl"` is replaced by the
//! preprocessed content of `name.wgsl`, resolved in the source directory.
//! Each file is inserted at most once per root file (the root counts), so
//! shared helpers may be included from several places and cycles end.
//!
//! Std only: `build.rs` uses this file through `#[path]`.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

/// A failed include: unreadable file or malformed directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprocessError {
    /// File holding the faulty directive, or the unreadable root.
    pub file: String,
    /// One-based line of the directive (0 for the root file).
    pub line: usize,
    pub message: String,
}

impl fmt::Display for PreprocessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.message)
    }
}

impl std::error::Error for PreprocessError {}

/// A root file with its includes expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preprocessed {
    pub source: String,
    /// Every file read, root first, in inclusion order.
    pub files: Vec<String>,
}

/// Expands the includes of `root`, reading files from `dir`.
pub fn preprocess_file(dir: &Path, root: &str) -> Result<Preprocessed, PreprocessError> {
    preprocess(root, |name| {
        let path: PathBuf = dir.join(name);
        std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    })
}

/// Expands the includes of `root`; `read` returns a file's text by name.
pub fn preprocess(
    root: &str,
    mut read: impl FnMut(&str) -> Result<String, String>,
) -> Result<Preprocessed, PreprocessError> {
    let mut seen = HashSet::new();
    let mut out = Preprocessed {
        source: String::new(),
        files: Vec::new(),
    };
    let text = read(root).map_err(|message| PreprocessError {
        file: root.to_owned(),
        line: 0,
        message,
    })?;
    seen.insert(root.to_owned());
    out.files.push(root.to_owned());
    expand(root, &text, &mut read, &mut seen, &mut out)?;
    Ok(out)
}

fn expand(
    file: &str,
    text: &str,
    read: &mut impl FnMut(&str) -> Result<String, String>,
    seen: &mut HashSet<String>,
    out: &mut Preprocessed,
) -> Result<(), PreprocessError> {
    for (index, line) in text.lines().enumerate() {
        let error = |message: String| PreprocessError {
            file: file.to_owned(),
            line: index + 1,
            message,
        };
        let Some(rest) = line.trim().strip_prefix("#include") else {
            out.source.push_str(line);
            out.source.push('\n');
            continue;
        };
        let name = parse_name(rest).ok_or_else(|| {
            error(format!(
                "malformed directive `{}`, expected #include \"name.wgsl\"",
                line.trim()
            ))
        })?;
        if name.split(['/', '\\']).any(|part| part == "..") || Path::new(name).is_absolute() {
            return Err(error(format!(
                "include `{name}` leaves the shader directory"
            )));
        }
        if !seen.insert(name.to_owned()) {
            continue;
        }
        let included = read(name).map_err(error)?;
        out.files.push(name.to_owned());
        expand(name, &included, read, seen, out)?;
    }
    Ok(())
}

/// `"name"` (surrounding spaces allowed) -> `name`.
fn parse_name(rest: &str) -> Option<&str> {
    let rest = rest.trim();
    let inner = rest.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.is_empty() && !inner.contains('"')).then_some(inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn files(list: &[(&str, &str)]) -> HashMap<String, String> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn run(map: &HashMap<String, String>, root: &str) -> Result<Preprocessed, PreprocessError> {
        preprocess(root, |name| {
            map.get(name)
                .cloned()
                .ok_or_else(|| format!("no file {name}"))
        })
    }

    #[test]
    fn includes_are_inlined_once() {
        let map = files(&[
            (
                "root.wgsl",
                "#include \"a.wgsl\"\n  #include \"b.wgsl\"  \nmain\n",
            ),
            ("a.wgsl", "#include \"common.wgsl\"\na\n"),
            (
                "b.wgsl",
                "#include \"common.wgsl\"\n#include \"root.wgsl\"\nb\n",
            ),
            ("common.wgsl", "common\n"),
        ]);
        let p = run(&map, "root.wgsl").unwrap();
        assert_eq!(p.source, "common\na\nb\nmain\n");
        assert_eq!(p.files, ["root.wgsl", "a.wgsl", "common.wgsl", "b.wgsl"]);
    }

    #[test]
    fn errors_name_the_file_and_line() {
        let map = files(&[
            ("root.wgsl", "x\n#include \"missing.wgsl\"\n"),
            ("bad.wgsl", "#include missing.wgsl\n"),
            ("escape.wgsl", "#include \"../secret.wgsl\"\n"),
        ]);
        let e = run(&map, "root.wgsl").unwrap_err();
        assert_eq!((e.file.as_str(), e.line), ("root.wgsl", 2));
        assert!(e
            .to_string()
            .starts_with("root.wgsl:2: no file missing.wgsl"));
        let e = run(&map, "bad.wgsl").unwrap_err();
        assert!(e.message.contains("malformed"));
        let e = run(&map, "escape.wgsl").unwrap_err();
        assert!(e.message.contains("leaves"));
        let e = run(&map, "absent.wgsl").unwrap_err();
        assert_eq!(e.line, 0);
    }
}
