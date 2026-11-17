//! Writing imported endpoint definitions to disk (shared by the OpenAPI,
//! Postman and Bruno importers).

use crate::config::Project;
use crate::model::EndpointDef;
use crate::naming;
use crate::project::load_endpoints;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub const YAML_SCHEMA_HEADER: &str = "# yaml-language-server: $schema=https://raw.githubusercontent.com/niti-lab/ajiro-api/main/schema/endpoint.schema.json\n";

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    /// Files (relative to the project root) that were created.
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    /// Variables found in the source collection: environment name -> name -> value.
    /// Suggested content for `environments.<env>.variables` in ajiro-api.yaml.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub suggested_variables: IndexMap<String, IndexMap<String, Value>>,
}

/// Serialize an endpoint definition as a YAML file with the schema header.
pub fn to_yaml(def: &EndpointDef) -> Result<String> {
    let body = serde_yaml_rust2::to_string(def).context("cannot serialize endpoint")?;
    Ok(format!("{}{}", YAML_SCHEMA_HEADER, body))
}

fn key(method: &str, path: &str) -> (String, String) {
    (method.to_ascii_uppercase(), naming::normalize_path(path))
}

/// Write `defs` into the project's api directory.
///
/// - Existing file with the same METHOD + path: untouched (`skipped`) unless
///   `overwrite`, in which case request/mock/summary/description are replaced
///   while `id`, `tests`, `source`, `depends_on`, `task` are kept.
/// - New endpoints get a unique id and the location from
///   [`naming::endpoint_rel_path`].
pub fn write_endpoints(
    project: &Project,
    defs: Vec<EndpointDef>,
    overwrite: bool,
    report: &mut ImportReport,
) -> Result<()> {
    let api_dir = project.api_dir();
    let (existing, _errs) = load_endpoints(project);
    let mut taken: HashSet<String> = existing.iter().map(|e| e.def.id.clone()).collect();
    let mut by_key = HashMap::new();
    for e in &existing {
        by_key.insert(key(&e.def.request.method, &e.def.request.path), e.clone());
    }
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for mut def in defs {
        let k = key(&def.request.method, &def.request.path);
        def.request.method = k.0.clone();
        def.request.path = k.1.clone();
        if !seen.insert(k.clone()) {
            report.warnings.push(format!(
                "duplicate {} {} in the source; only the first one was imported",
                k.0, k.1
            ));
            continue;
        }
        if let Some(old) = by_key.get(&k) {
            if !overwrite {
                report.skipped.push(old.rel_path.clone());
                continue;
            }
            let mut merged = old.def.clone();
            merged.request = def.request;
            merged.mock = def.mock;
            if def.summary.is_some() {
                merged.summary = def.summary;
            }
            if def.description.is_some() {
                merged.description = def.description;
            }
            if merged.tags.is_empty() {
                merged.tags = def.tags;
            }
            if merged.tests.is_empty() {
                merged.tests = def.tests;
            }
            std::fs::write(&old.path, to_yaml(&merged)?)
                .with_context(|| format!("cannot write {}", old.path.display()))?;
            report.updated.push(old.rel_path.clone());
            continue;
        }
        def.id = naming::unique_id(&def.id, &taken);
        taken.insert(def.id.clone());
        // Different paths can map to the same file name
        // (`/a/b-c` and `/a/b/c` -> `post-a-b-c.yaml`): add `-2`, `-3`, ...
        let path = unique_file(api_dir.join(naming::endpoint_rel_path(
            &def.request.method,
            &def.request.path,
        )));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        std::fs::write(&path, to_yaml(&def)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
        report.created.push(project.rel(&path));
    }
    Ok(())
}

/// `path` if it does not exist yet, else `<stem>-2.yaml`, `<stem>-3.yaml`, ...
fn unique_file(path: std::path::PathBuf) -> std::path::PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "yaml".into());
    (2..)
        .map(|n| path.with_file_name(format!("{}-{}.{}", stem, n, ext)))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

/// Print the report as JSON or as a short human summary.
pub fn print_report(report: &ImportReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!(
        "{} created, {} updated, {} skipped",
        report.created.len(),
        report.updated.len(),
        report.skipped.len()
    );
    for f in &report.created {
        println!("  + {}", f);
    }
    for f in &report.updated {
        println!("  ~ {}", f);
    }
    for f in &report.skipped {
        println!("  = {} (exists, use --overwrite to replace)", f);
    }
    for w in &report.warnings {
        println!("warning: {}", w);
    }
    if !report.suggested_variables.is_empty() {
        println!("\nSuggested variables for ajiro-api.yaml:");
        let mut envs = serde_json::Map::new();
        for (env, vars) in &report.suggested_variables {
            let vars: serde_json::Map<String, Value> =
                vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            envs.insert(env.clone(), serde_json::json!({ "variables": vars }));
        }
        let y = serde_yaml_rust2::to_string(&serde_json::json!({ "environments": envs }))
            .unwrap_or_default();
        for l in y.lines() {
            println!("  {}", l);
        }
    }
    Ok(())
}
