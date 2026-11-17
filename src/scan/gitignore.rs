//! `.gitignore` support for the file walk.
//!
//! Applies the `.gitignore` files from the repository root down to the
//! scanned directory (and nested ones), like git does, with one exception
//! that git also makes: files that are *tracked* in the index are never
//! ignored (a pattern such as `[internal]` can match a tracked directory
//! named `t`). Outside a git repository nothing is ignored.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct GitFilter {
    repo: PathBuf,
    matchers: HashMap<PathBuf, Option<Gitignore>>,
    tracked_files: HashSet<String>,
    tracked_dirs: HashSet<String>,
}

impl GitFilter {
    /// `None` when `start` is not inside a git repository.
    pub fn discover(start: &Path) -> Option<GitFilter> {
        let repo = start
            .ancestors()
            .find(|a| a.join(".git").exists())?
            .to_path_buf();
        let mut tracked_files = HashSet::new();
        let mut tracked_dirs = HashSet::new();
        if let Ok(bytes) = std::fs::read(repo.join(".git").join("index")) {
            for p in parse_index(&bytes).unwrap_or_default() {
                let mut end = p.len();
                while let Some(i) = p[..end].rfind('/') {
                    tracked_dirs.insert(p[..i].to_string());
                    end = i;
                }
                tracked_files.insert(p);
            }
        }
        Some(GitFilter {
            repo,
            matchers: HashMap::new(),
            tracked_files,
            tracked_dirs,
        })
    }

    fn matcher(&mut self, dir: &Path) -> Option<&Gitignore> {
        self.matchers
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                let f = dir.join(".gitignore");
                if !f.is_file() {
                    return None;
                }
                let mut b = GitignoreBuilder::new(dir);
                b.add(f);
                b.build().ok()
            })
            .as_ref()
    }

    /// Whether git would ignore `path` (and it is not tracked).
    pub fn is_ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        let Ok(rel) = path.strip_prefix(&self.repo) else {
            return false;
        };
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        let dirs: Vec<PathBuf> = path
            .ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(&self.repo))
            .map(Path::to_path_buf)
            .collect();
        let mut ignored = false;
        // Deepest `.gitignore` first: it overrides the ones above it.
        for d in dirs {
            let verdict = self.matcher(&d).map(|m| m.matched(path, is_dir));
            match verdict {
                Some(Match::Ignore(_)) => {
                    ignored = true;
                    break;
                }
                Some(Match::Whitelist(_)) => break,
                _ => {}
            }
        }
        ignored
            && !(if is_dir {
                self.tracked_dirs.contains(&rel_s)
            } else {
                self.tracked_files.contains(&rel_s)
            })
    }
}

/// Paths in a git index file (versions 2, 3 and 4).
fn parse_index(b: &[u8]) -> Option<Vec<String>> {
    if b.len() < 12 || &b[..4] != b"DIRC" {
        return None;
    }
    let be32 =
        |o: usize| -> Option<u32> { Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    let version = be32(4)?;
    let count = be32(8)? as usize;
    let mut out = Vec::with_capacity(count);
    let mut pos = 12;
    let mut prev = String::new();
    for _ in 0..count {
        let flags = u16::from_be_bytes(b.get(pos + 60..pos + 62)?.try_into().ok()?);
        let mut p = pos + 62;
        if version >= 3 && flags & 0x4000 != 0 {
            p += 2;
        }
        let path;
        if version == 4 {
            let (mut strip, mut shift) = (0usize, 0u32);
            // offset-encoded varint
            let mut first = true;
            loop {
                let c = *b.get(p)?;
                p += 1;
                if first {
                    strip = (c & 0x7f) as usize;
                    first = false;
                } else {
                    strip = ((strip + 1) << 7) | (c & 0x7f) as usize;
                }
                shift += 7;
                if c & 0x80 == 0 || shift > 56 {
                    break;
                }
            }
            let nul = b.get(p..)?.iter().position(|&c| c == 0)?;
            let keep = prev.len().checked_sub(strip)?;
            path = format!(
                "{}{}",
                prev.get(..keep)?,
                String::from_utf8_lossy(&b[p..p + nul])
            );
            pos = p + nul + 1;
        } else {
            let len = (flags & 0xfff) as usize;
            let len = if len == 0xfff {
                b.get(p..)?.iter().position(|&c| c == 0)?
            } else {
                len
            };
            path = String::from_utf8_lossy(b.get(p..p + len)?).into_owned();
            pos += (p - pos + len + 8) & !7;
        }
        prev = path.clone();
        out.push(path);
    }
    Some(out)
}
