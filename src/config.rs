//! Locating and loading `ajiro-api.yaml`.

use crate::model::{ProjectConfig, CONFIG_FILE};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// A loaded project: root directory + parsed config.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub config: ProjectConfig,
}

impl Project {
    /// Absolute path of the endpoint directory.
    pub fn api_dir(&self) -> PathBuf {
        self.root.join(&self.config.api_dir)
    }

    /// Absolute paths of the source directories to scan.
    pub fn source_dirs(&self) -> Vec<PathBuf> {
        self.config
            .sources
            .iter()
            .map(|s| self.root.join(s))
            .collect()
    }

    /// Path relative to the project root, `/`-separated (for YAML / output).
    pub fn rel(&self, p: &Path) -> String {
        let r = p.strip_prefix(&self.root).unwrap_or(p);
        r.to_string_lossy().replace('\\', "/")
    }

    /// Discover the project starting at `start` (walks up to the filesystem
    /// root looking for `ajiro-api.yaml`).
    pub fn discover(start: &Path) -> Result<Project> {
        let start = start
            .canonicalize()
            .with_context(|| format!("cannot access {}", start.display()))?;
        if let Some(candidate) = Project::find_config(&start) {
            return Project::load(&candidate);
        }
        bail!(
            "{} not found in {} or any parent directory. Run `ajiro-api init` first.",
            CONFIG_FILE,
            start.display()
        )
    }

    /// Path of the nearest `ajiro-api.yaml` at or above `start`.
    fn find_config(start: &Path) -> Option<PathBuf> {
        start
            .ancestors()
            .map(|d| d.join(CONFIG_FILE))
            .find(|c| c.is_file())
    }

    /// Load the config file at `path`.
    pub fn load(path: &Path) -> Result<Project> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let config: ProjectConfig = serde_yaml_rust2::from_str(&text)
            .with_context(|| format!("invalid {}", path.display()))?;
        if config.version != crate::model::SCHEMA_VERSION {
            bail!(
                "{}: unsupported version {} (this ajiro-api supports {})",
                path.display(),
                config.version,
                crate::model::SCHEMA_VERSION
            );
        }
        let root = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Project { root, config })
    }

    /// Discover the project, or fall back to an in-memory default config
    /// rooted at `start` (used by commands that can work without init).
    ///
    /// The fallback applies only when no `ajiro-api.yaml` exists. A config
    /// that exists but is invalid is an error, never silently ignored.
    pub fn discover_or_default(start: &Path) -> Result<Project> {
        let canon = start.canonicalize().ok();
        if let Some(c) = canon.as_deref().and_then(Project::find_config) {
            return Project::load(&c);
        }
        Ok(Project {
            root: canon.unwrap_or_else(|| start.to_path_buf()),
            config: ProjectConfig::default(),
        })
    }
}
