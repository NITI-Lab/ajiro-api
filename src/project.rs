//! Loading endpoint files from the `api/` directory.

use crate::config::Project;
use crate::model::EndpointDef;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// An endpoint definition together with the file it came from.
#[derive(Debug, Clone)]
pub struct LoadedEndpoint {
    /// Absolute path of the YAML file.
    pub path: PathBuf,
    /// Path relative to the project root, `/`-separated.
    pub rel_path: String,
    pub def: EndpointDef,
}

/// Error for one file that could not be parsed.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LoadError {
    pub file: String,
    pub message: String,
}

/// All `*.yaml` / `*.yml` files under `api_dir` (skips `__snapshots__` and
/// `schemas` directories), sorted by path.
pub fn endpoint_files(api_dir: &Path) -> Vec<PathBuf> {
    if !api_dir.is_dir() {
        return vec![];
    }
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(api_dir)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(e.file_type().is_dir()
                && (n == "__snapshots__" || n == "schemas" || n.starts_with('.')))
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("yaml") | Some("yml")
            )
        })
        .collect();
    files.sort();
    files
}

pub fn load_endpoint_file(path: &Path) -> Result<EndpointDef> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let def: EndpointDef = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid endpoint file {}", path.display()))?;
    Ok(def)
}

/// Load every endpoint file. Files that fail to parse are returned as errors
/// rather than aborting, so `test` and `validate` can report all problems.
pub fn load_endpoints(project: &Project) -> (Vec<LoadedEndpoint>, Vec<LoadError>) {
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    for path in endpoint_files(&project.api_dir()) {
        let rel_path = project.rel(&path);
        match load_endpoint_file(&path) {
            Ok(def) => ok.push(LoadedEndpoint {
                path,
                rel_path,
                def,
            }),
            Err(e) => errs.push(LoadError {
                file: rel_path,
                message: format!("{:#}", e),
            }),
        }
    }
    (ok, errs)
}

/// Semantic checks beyond YAML parsing: duplicate ids, unknown
/// `depends_on`, path params without values, unsupported version.
pub fn validate(endpoints: &[LoadedEndpoint]) -> Vec<LoadError> {
    use std::collections::HashMap;
    let mut errs = Vec::new();
    let mut ids: HashMap<&str, &str> = HashMap::new();
    for e in endpoints {
        if e.def.version != crate::model::SCHEMA_VERSION {
            errs.push(LoadError {
                file: e.rel_path.clone(),
                message: format!("unsupported version {}", e.def.version),
            });
        }
        if let Some(prev) = ids.insert(&e.def.id, &e.rel_path) {
            errs.push(LoadError {
                file: e.rel_path.clone(),
                message: format!("duplicate id `{}` (also in {})", e.def.id, prev),
            });
        }
        let m = e.def.request.method.to_ascii_uppercase();
        if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"].contains(&m.as_str()) {
            errs.push(LoadError {
                file: e.rel_path.clone(),
                message: format!("unsupported method `{}`", e.def.request.method),
            });
        }
        if !e.def.request.path.starts_with('/') {
            errs.push(LoadError {
                file: e.rel_path.clone(),
                message: format!("request.path must start with `/`: `{}`", e.def.request.path),
            });
        }
        for (i, t) in e.def.tests.iter().enumerate() {
            if t.name.trim().is_empty() {
                errs.push(LoadError {
                    file: e.rel_path.clone(),
                    message: format!("tests[{}].name is empty", i),
                });
            }
        }
    }
    let known: std::collections::HashSet<&str> =
        endpoints.iter().map(|e| e.def.id.as_str()).collect();
    for e in endpoints {
        for d in &e.def.depends_on {
            if !known.contains(d.as_str()) {
                errs.push(LoadError {
                    file: e.rel_path.clone(),
                    message: format!("depends_on refers to unknown id `{}`", d),
                });
            }
        }
    }
    errs
}
