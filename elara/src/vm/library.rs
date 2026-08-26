//! Logical import names, relative resolution, and filesystem candidate search.

/// One module activation. `search_root` is `None` when a non-file module has no entry anchor.
#[derive(Clone, Debug)]
pub struct LoadFrame {
    pub search_root: Option<String>,
    pub rel_dir: String,
}

/// Why a specifier was rejected before any search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecFail {
    Traversal,
    Absolute,
    Invalid,
    NeedsAnchor,
}

/// Normalized module name. `only_root` is set for a relative import.
#[derive(Clone, Debug)]
pub struct LogicalSpec {
    pub key: String,
    pub only_root: Option<String>,
}

/// A file chosen from the search path.
pub struct DiskModule {
    pub bytes: Vec<u8>,
    pub is_bytecode: bool,
    pub is_native: bool,
    pub path: String,
    pub search_root: String,
    pub rel_dir: String,
}

/// Frame that relative imports see. An empty stack falls back to the entry anchor.
#[must_use]
pub fn active_frame(anchor: Option<&str>, stack: &[LoadFrame]) -> Option<LoadFrame> {
    if let Some(top) = stack.last() {
        return Some(top.clone());
    }
    anchor.map(|root| LoadFrame {
        search_root: Some(root.to_string()),
        rel_dir: String::new(),
    })
}

/// Classifies `spec` against `frame` (required for `./` and `../`).
pub fn classify(spec: &str, frame: Option<&LoadFrame>) -> Result<LogicalSpec, SpecFail> {
    if spec.contains('\0') {
        return Err(SpecFail::Invalid);
    }
    if spec.starts_with('/') {
        return Err(SpecFail::Absolute);
    }
    // An empty segment (`mem://name`) stays a logical name so a host loader can use a
    // virtual specifier. Relative specifiers still reject empty segments below.
    if spec.is_empty() || spec.contains('\\') {
        return Err(SpecFail::Traversal);
    }
    if spec.starts_with("./") || spec.starts_with("../") {
        let frame = frame.ok_or(SpecFail::NeedsAnchor)?;
        let root = frame.search_root.as_deref().ok_or(SpecFail::NeedsAnchor)?;
        let key = resolve_relative(&frame.rel_dir, spec)?;
        return Ok(LogicalSpec {
            key,
            only_root: Some(root.to_string()),
        });
    }
    if spec.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(SpecFail::Traversal);
    }
    let key = strip_module_suffix(spec);
    if key.is_empty() {
        return Err(SpecFail::Traversal);
    }
    Ok(LogicalSpec {
        key,
        only_root: None,
    })
}

fn resolve_relative(rel_dir: &str, spec: &str) -> Result<String, SpecFail> {
    let mut parts: Vec<&str> = Vec::new();
    if !rel_dir.is_empty() {
        for seg in rel_dir.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(SpecFail::Traversal);
            }
            parts.push(seg);
        }
    }
    for seg in spec.split('/') {
        if seg.is_empty() {
            return Err(SpecFail::Traversal);
        }
        if seg == "." {
            continue;
        }
        if seg == ".." {
            if parts.pop().is_none() {
                return Err(SpecFail::Traversal);
            }
            continue;
        }
        parts.push(seg);
    }
    if parts.is_empty() {
        return Err(SpecFail::Traversal);
    }
    let key = strip_module_suffix(&parts.join("/"));
    if key.is_empty() {
        Err(SpecFail::Traversal)
    } else {
        Ok(key)
    }
}

pub(crate) fn strip_module_suffix(key: &str) -> String {
    let (dir, base) = match key.rfind('/') {
        Some(i) => (&key[..i], &key[i + 1..]),
        None => ("", key),
    };
    let base = base
        .strip_suffix(".elac")
        .or_else(|| base.strip_suffix(".ela"))
        .unwrap_or(base);
    if dir.is_empty() {
        base.to_string()
    } else if base.is_empty() {
        String::new()
    } else {
        format!("{dir}/{base}")
    }
}

/// Parent directory of a logical name (`vec/geom` → `vec`, `vec` → empty).
#[must_use]
pub fn parent_key(logical: &str) -> String {
    match logical.rfind('/') {
        Some(i) => logical[..i].to_string(),
        None => String::new(),
    }
}

/// First existing candidate: `.elac`, `.ela`, `init.elac`, `init.ela`, then `.so`.
///
/// `files` and `native` select which of those groups are visible. `only_root`
/// limits the search to one directory (relative imports). An empty `dirs` list
/// with no `only_root` searches `.`.
#[must_use]
pub fn find_module(
    dirs: &[String],
    logical: &str,
    only_root: Option<&str>,
    files: bool,
    native: bool,
) -> Option<DiskModule> {
    if !files && !native {
        return None;
    }
    let fallback;
    let search: &[String] = if let Some(root) = only_root {
        fallback = vec![root.to_string()];
        &fallback
    } else if dirs.is_empty() {
        fallback = vec![".".to_string()];
        &fallback
    } else {
        dirs
    };
    let names = [
        (format!("{logical}.elac"), false),
        (format!("{logical}.ela"), false),
        (format!("{logical}/init.elac"), true),
        (format!("{logical}/init.ela"), true),
    ];
    for dir in search {
        if files {
            for (name, is_init) in &names {
                let path = join_dir(dir, name);
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let is_bytecode = bytes.len() >= 6 && bytes[0] == 0x1B && bytes[1..6] == *b"ELARA";
                let rel_dir = if *is_init {
                    logical.to_string()
                } else {
                    parent_key(logical)
                };
                return Some(DiskModule {
                    bytes,
                    is_bytecode,
                    is_native: false,
                    path,
                    search_root: dir.clone(),
                    rel_dir,
                });
            }
        }
        if native {
            let path = join_dir(dir, &format!("{logical}.so"));
            if std::fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
                return Some(DiskModule {
                    bytes: Vec::new(),
                    is_bytecode: false,
                    is_native: true,
                    path,
                    search_root: dir.clone(),
                    rel_dir: parent_key(logical),
                });
            }
        }
    }
    None
}

fn join_dir(dir: &str, name: &str) -> String {
    if dir == "." {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}
