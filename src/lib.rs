// SPDX-License-Identifier: MIT
// SigmaOS-plocate - Indexed File Name Search component
// plocate-style fast filename search using a prebuilt index database,
// inspired by Arch's plocate package (locate/mlocate successor).

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Mutex;

// ============================================================================
// INDEX ENTRY
// ============================================================================

/// An entry in the locate database
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// Full path
    pub path: PathBuf,
    /// File size in bytes (0 for dirs)
    pub size: u64,
    /// Last modification time (Unix timestamp)
    pub mtime: u64,
    /// Whether this is a directory
    pub is_dir: bool,
    /// Whether this is a symlink
    pub is_symlink: bool,
    /// Permissions (mode bits)
    pub mode: u32,
}

impl IndexEntry {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            size: 0,
            mtime: 0,
            is_dir: false,
            is_symlink: false,
            mode: 0o644,
        }
    }
}

// ============================================================================
// SEARCH QUERY
// ============================================================================

/// Search pattern for locate
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// Pattern to match (supports glob *)
    pub pattern: String,
    /// Whether match is case-sensitive
    pub case_sensitive: bool,
    /// Whether to match full path or basename only
    pub basename_only: bool,
    /// Maximum results to return
    pub limit: usize,
}

impl SearchQuery {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            case_sensitive: false,
            basename_only: false,
            limit: 1000,
        }
    }

    pub fn case_sensitive(mut self, yes: bool) -> Self {
        self.case_sensitive = yes;
        self
    }

    pub fn basename_only(mut self, yes: bool) -> Self {
        self.basename_only = yes;
        self
    }

    pub fn limit(mut self, n: usize) -> Self {
        self.limit = n;
        self
    }

    /// Convert glob pattern to regex-like matcher
    fn matches(&self, path: &PathBuf) -> bool {
        let path_str = if self.basename_only {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            path.to_string_lossy().to_string()
        };

        let target = if self.case_sensitive {
            path_str
        } else {
            path_str.to_lowercase()
        };
        let mut pattern = if self.case_sensitive {
            self.pattern.clone()
        } else {
            self.pattern.to_lowercase()
        };
        // locate matches substring anywhere in path
        if !self.basename_only && !pattern.starts_with('*') {
            pattern = format!("*/{}", pattern);
        }

        glob_match(&pattern, &target)
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    // Simple glob: * matches any chars, ? matches single char
    fn inner(p: &[u8], t: &[u8]) -> bool {
        if p.is_empty() { return t.is_empty(); }
        match p[0] {
            b'*' => {
                // * matches zero or more chars
                for i in 0..=t.len() {
                    if inner(&p[1..], &t[i..]) { return true; }
                }
                false
            }
            b'?' => !t.is_empty() && inner(&p[1..], &t[1..]),
            c => !t.is_empty() && t[0] == c && inner(&p[1..], &t[1..]),
        }
    }
    inner(pattern.as_bytes(), text.as_bytes())
}

// ============================================================================
// LOCATE DATABASE
// ============================================================================

/// The locate database (prebuilt file index)
pub struct LocateDb {
    /// All indexed paths
    entries: Mutex<Vec<IndexEntry>>,
    /// Index by directory prefix for faster lookup
    dir_index: Mutex<HashMap<String, Vec<usize>>>,
    /// Whether database is built
    built: Mutex<bool>,
    /// Database path
    db_path: PathBuf,
    /// Number of entries
    entry_count: Mutex<usize>,
}

impl LocateDb {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            dir_index: Mutex::new(HashMap::new()),
            built: Mutex::new(false),
            db_path,
            entry_count: Mutex::new(0),
        }
    }

    /// Default database path
    pub fn default_path() -> PathBuf {
        PathBuf::from("/var/lib/plocate/plocate.db")
    }

    /// Add an entry to the database
    pub fn add(&self, entry: IndexEntry) {
        let mut entries = self.entries.lock().unwrap();
        let idx = entries.len();
        // Index by parent directory
        if let Some(parent) = entry.path.parent() {
            let dir = parent.to_string_lossy().to_string();
            self.dir_index.lock().unwrap()
                .entry(dir).or_default().push(idx);
        }
        entries.push(entry);
        *self.entry_count.lock().unwrap() = entries.len();
    }

    /// Bulk add entries
    pub fn add_all(&self, entries: Vec<IndexEntry>) {
        for entry in entries {
            self.add(entry);
        }
    }

    /// Search the database
    pub fn search(&self, query: &SearchQuery) -> Vec<PathBuf> {
        if !*self.built.lock().unwrap() {
            return Vec::new();
        }
        let entries = self.entries.lock().unwrap();
        let mut results = Vec::new();
        for entry in entries.iter() {
            if query.matches(&entry.path) {
                results.push(entry.path.clone());
                if results.len() >= query.limit {
                    break;
                }
            }
        }
        results
    }

    /// Search with full entry info
    pub fn search_entries(&self, query: &SearchQuery) -> Vec<IndexEntry> {
        if !*self.built.lock().unwrap() {
            return Vec::new();
        }
        let entries = self.entries.lock().unwrap();
        let mut results = Vec::new();
        for entry in entries.iter() {
            if query.matches(&entry.path) {
                results.push(entry.clone());
                if results.len() >= query.limit {
                    break;
                }
            }
        }
        results
    }

    /// Mark database as built
    pub fn mark_built(&self) {
        *self.built.lock().unwrap() = true;
    }

    /// Check if database is built
    pub fn is_built(&self) -> bool {
        *self.built.lock().unwrap()
    }

    /// Get entry count
    pub fn count(&self) -> usize {
        *self.entry_count.lock().unwrap()
    }

    /// Clear the database
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
        self.dir_index.lock().unwrap().clear();
        *self.entry_count.lock().unwrap() = 0;
        *self.built.lock().unwrap() = false;
    }

    /// Get database path
    pub fn path(&self) -> &PathBuf {
        &self.db_path
    }

    /// Estimate index size in bytes
    pub fn size_bytes(&self) -> usize {
        let entries = self.entries.lock().unwrap();
        entries.iter().map(|e| e.path.to_string_lossy().len() + 32).sum()
    }
}

// ============================================================================
// DATABASE BUILDER
// ============================================================================

/// Builds the locate database by scanning directories
pub struct DbBuilder {
    /// Directories to scan
    pub scan_paths: Vec<PathBuf>,
    /// Paths to exclude
    pub exclude_paths: Vec<PathBuf>,
    /// Whether to follow symlinks
    pub follow_symlinks: bool,
    /// Whether to include hidden files
    pub include_hidden: bool,
}

impl DbBuilder {
    pub fn new() -> Self {
        Self {
            scan_paths: vec![
                PathBuf::from("/usr"),
                PathBuf::from("/bin"),
                PathBuf::from("/sbin"),
                PathBuf::from("/etc"),
                PathBuf::from("/var"),
                PathBuf::from("/opt"),
            ],
            exclude_paths: vec![
                PathBuf::from("/proc"),
                PathBuf::from("/sys"),
                PathBuf::from("/dev"),
                PathBuf::from("/run"),
                PathBuf::from("/tmp"),
            ],
            follow_symlinks: false,
            include_hidden: false,
        }
    }

    /// Whether a path should be excluded
    pub fn is_excluded(&self, path: &PathBuf) -> bool {
        self.exclude_paths.iter().any(|ex| path.starts_with(ex))
    }

    /// Whether a filename should be included
    pub fn is_included(&self, name: &str) -> bool {
        if !self.include_hidden && name.starts_with('.') {
            return false;
        }
        true
    }

    /// Estimate number of files to scan
    pub fn estimate_entries(&self) -> usize {
        // Rough estimate: ~500K files in a typical system
        500_000
    }
}

impl Default for DbBuilder {
    fn default() -> Self { Self::new() }
}

// ============================================================================
// ERRORS
// ============================================================================

#[derive(Debug)]
pub enum LocateError {
    DbNotBuilt,
    Io(String),
    PermissionDenied,
}

impl fmt::Display for LocateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DbNotBuilt => write!(f, "database not built"),
            Self::Io(m) => write!(f, "io error: {}", m),
            Self::PermissionDenied => write!(f, "permission denied"),
        }
    }
}
impl std::error::Error for LocateError {}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "src/main.rs"));
        assert!(!glob_match("*.rs", "main.c"));
        assert!(glob_match("foo?bar", "fooXbar"));
        assert!(!glob_match("foo?bar", "foobar"));
        assert!(glob_match("*", "anything"));
    }

    #[test]
    fn test_search_query() {
        let db = LocateDb::new(PathBuf::from("/tmp/test.db"));
        db.add(IndexEntry::new(PathBuf::from("/usr/bin/bash")));
        db.add(IndexEntry::new(PathBuf::from("/usr/bin/zsh")));
        db.add(IndexEntry::new(PathBuf::from("/etc/bashrc")));
        db.mark_built();

        let results = db.search(&SearchQuery::new("*bash*"));
        assert_eq!(results.len(), 2);
        assert!(results.contains(&PathBuf::from("/usr/bin/bash")));
        assert!(results.contains(&PathBuf::from("/etc/bashrc")));
    }

    #[test]
    fn test_basename_search() {
        let db = LocateDb::new(PathBuf::from("/tmp/test.db"));
        db.add(IndexEntry::new(PathBuf::from("/usr/bin/bash")));
        db.add(IndexEntry::new(PathBuf::from("/etc/bashrc")));
        db.mark_built();

        let results = db.search(&SearchQuery::new("bash").basename_only(true));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], PathBuf::from("/usr/bin/bash"));
    }

    #[test]
    fn test_case_insensitive() {
        let db = LocateDb::new(PathBuf::from("/tmp/test.db"));
        db.add(IndexEntry::new(PathBuf::from("/usr/bin/BASH")));
        db.mark_built();

        let results = db.search(&SearchQuery::new("bash"));
        assert_eq!(results.len(), 1);

        let results = db.search(&SearchQuery::new("bash").case_sensitive(true));
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_limit() {
        let db = LocateDb::new(PathBuf::from("/tmp/test.db"));
        for i in 0..100 {
            db.add(IndexEntry::new(PathBuf::from(format!("/usr/bin/tool{}", i))));
        }
        db.mark_built();

        let results = db.search(&SearchQuery::new("tool*").limit(10));
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_db_not_built() {
        let db = LocateDb::new(PathBuf::from("/tmp/test.db"));
        db.add(IndexEntry::new(PathBuf::from("/usr/bin/bash")));
        // Not marked as built
        assert_eq!(db.search(&SearchQuery::new("bash")).len(), 0);
        db.mark_built();
        assert_eq!(db.search(&SearchQuery::new("bash")).len(), 1);
    }

    #[test]
    fn test_builder_exclusions() {
        let builder = DbBuilder::new();
        assert!(builder.is_excluded(&PathBuf::from("/proc/cpuinfo")));
        assert!(builder.is_excluded(&PathBuf::from("/sys/kernel")));
        assert!(!builder.is_excluded(&PathBuf::from("/usr/bin/bash")));
        assert!(!builder.is_included(".hidden"));
        assert!(builder.is_included("visible"));
    }

    #[test]
    fn test_entry_count() {
        let db = LocateDb::new(PathBuf::from("/tmp/test.db"));
        db.add(IndexEntry::new(PathBuf::from("/a")));
        db.add(IndexEntry::new(PathBuf::from("/b")));
        assert_eq!(db.count(), 2);
        db.clear();
        assert_eq!(db.count(), 0);
    }
}
