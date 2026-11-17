//! Per-process cache of directory listings.
//!
//! Scanners look for marker files (`tsconfig.json`, `next.config.*`,
//! `.gitignore`, ...) in many ancestor directories. On network and WSL
//! (`/mnt/c`) mounts every `stat` costs milliseconds, so each directory is
//! listed once and later lookups are answered from memory.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

thread_local! {
    static LISTINGS: RefCell<HashMap<PathBuf, Rc<HashSet<String>>>> = RefCell::new(HashMap::new());
}

/// File and directory names directly inside `dir` (empty when unreadable).
pub fn names(dir: &Path) -> Rc<HashSet<String>> {
    if let Some(hit) = LISTINGS.with(|l| l.borrow().get(dir).cloned()) {
        return hit;
    }
    let set: HashSet<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let set = Rc::new(set);
    LISTINGS.with(|l| l.borrow_mut().insert(dir.to_path_buf(), set.clone()));
    set
}

/// Whether `dir` contains an entry called `name` (file or directory).
pub fn has(dir: &Path, name: &str) -> bool {
    names(dir).contains(name)
}

/// Forget every listing (tests that rewrite files between scans).
pub fn clear() {
    LISTINGS.with(|l| l.borrow_mut().clear());
}
