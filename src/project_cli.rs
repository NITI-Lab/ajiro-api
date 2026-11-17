//! `list` and `validate` commands.

use crate::cli::{ListArgs, ValidateArgs};
use crate::config::Project;
use crate::project::{load_endpoints, validate as validate_endpoints};
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct ListItem {
    pub id: String,
    pub method: String,
    pub path: String,
    pub file: String,
    pub tests: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::model::SourceInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// Endpoint list for `list` and the MCP `list` tool.
pub fn list_items(project: &Project) -> (Vec<ListItem>, Vec<crate::project::LoadError>) {
    let (eps, errs) = load_endpoints(project);
    let items = eps
        .into_iter()
        .map(|e| ListItem {
            id: e.def.id,
            method: e.def.request.method.to_ascii_uppercase(),
            path: e.def.request.path,
            file: e.rel_path,
            tests: e.def.tests.len(),
            source: e.def.source,
            summary: e.def.summary,
            task: e.def.task,
        })
        .collect();
    (items, errs)
}

pub fn list(dir: &Path, args: &ListArgs) -> Result<i32> {
    let project = Project::discover(dir)?;
    let (items, errs) = list_items(&project);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "endpoints": items, "load_errors": errs })
            )?
        );
    } else {
        if items.is_empty() {
            println!(
                "No endpoints in {}/. Run `ajiro-api scan`.",
                project.config.api_dir
            );
        }
        let w = items.iter().map(|i| i.method.len()).max().unwrap_or(0);
        for i in &items {
            println!(
                "{:<w$}  {:<40}  {:<28}  tests:{}",
                i.method,
                i.path,
                i.id,
                i.tests,
                w = w
            );
        }
        for e in &errs {
            eprintln!("error: {}: {}", e.file, e.message);
        }
    }
    Ok(if errs.is_empty() {
        crate::exit::OK
    } else {
        crate::exit::FAILURE
    })
}

/// Load + validate everything. Returns all problems found.
pub fn validate_project(project: &Project) -> Vec<crate::project::LoadError> {
    let (eps, mut errs) = load_endpoints(project);
    errs.extend(validate_endpoints(&eps));
    let known: std::collections::HashSet<&str> = eps.iter().map(|e| e.def.id.as_str()).collect();
    for (name, env) in &project.config.environments {
        for id in &env.setup {
            if !known.contains(id.as_str()) {
                errs.push(crate::project::LoadError {
                    file: "ajiro-api.yaml".into(),
                    message: format!("environments.{}.setup refers to unknown id `{}`", name, id),
                });
            }
        }
    }
    errs
}

pub fn validate(dir: &Path, args: &ValidateArgs) -> Result<i32> {
    let project = Project::discover(dir)?;
    let errs = validate_project(&project);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "ok": errs.is_empty(), "errors": errs })
            )?
        );
    } else if errs.is_empty() {
        println!("All endpoint files are valid.");
    } else {
        for e in &errs {
            println!("{}: {}", e.file, e.message);
        }
        println!("{} problem(s) found.", errs.len());
    }
    Ok(if errs.is_empty() {
        crate::exit::OK
    } else {
        crate::exit::FAILURE
    })
}
