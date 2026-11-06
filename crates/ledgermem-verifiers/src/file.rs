//! The `file` verifier: checks that a file exists, contains some text, or has a SHA-256 hash.
//!
//! | `check` | Parameters | Passes when |
//! | --- | --- | --- |
//! | `exists` | `path` (default: the claim's value) | the path exists |
//! | `contains` | `path`, `pattern` (default: the claim's value) | the file contains `pattern` |
//! | `sha256` | `path` | the file's hash is the claim's value, with or without `sha256:` |
//!
//! Paths are relative to the verifier's root, and a path that would leave it, by `..`, by being
//! absolute, or through a link, is never read.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};

use ledgermem_core::{Check, Observation, Verifier, Verifying};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{NOTHING_TO_FIND, blocking, contains, needs, unsupported};

/// The largest file `contains` reads.
const MAX_SEARCHED: u64 = 64 * 1024 * 1024;

/// Checks files under one root directory.
#[derive(Debug, Clone)]
pub struct FileVerifier {
    root: PathBuf,
}

/// One file check, with everything it needs to run apart from the claim.
struct FileCheck {
    /// The path as the policy and claim name it.
    named: String,
    path: PathBuf,
    root: PathBuf,
    kind: Kind,
}

enum Kind {
    Exists,
    Contains(String),
    Sha256(String),
}

impl FileVerifier {
    /// A verifier that reads files under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        FileVerifier { root: root.into() }
    }

    fn file_check(&self, check: Check<'_>) -> Result<FileCheck, String> {
        let value = || check.expand("{value}");
        let path = || check.param("path").ok_or_else(|| needs("path"));
        let (named, kind) = match check.param("check").as_deref() {
            Some("exists") => (check.param("path").unwrap_or_else(value), Kind::Exists),
            Some("contains") => (
                path()?,
                Kind::Contains(check.param("pattern").unwrap_or_else(value)),
            ),
            Some("sha256") => (path()?, Kind::Sha256(value())),
            other => return Err(unsupported("file", "exists, contains or sha256", other)),
        };
        let relative = Path::new(&named);
        let stays_under_root = !named.is_empty()
            && relative
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
        if !stays_under_root {
            return Err(format!(
                "{named:?} isn't a path under the file verifier's root"
            ));
        }
        Ok(FileCheck {
            path: self.root.join(relative),
            root: self.root.clone(),
            named,
            kind,
        })
    }
}

impl Verifier for FileVerifier {
    fn verify<'a>(&'a self, check: Check<'a>) -> Verifying<'a> {
        let file_check = self.file_check(check);
        Box::pin(async move {
            let file_check = match file_check {
                Ok(file_check) => file_check,
                Err(why) => return Observation::inconclusive(why),
            };
            blocking("reading the file", move || file_check.observe())
                .await
                .unwrap_or_else(Observation::inconclusive)
        })
    }
}

impl FileCheck {
    fn observe(self) -> Observation {
        let named = &self.named;
        let exists = match self.resolve() {
            Ok(exists) => exists,
            Err(why) => return Observation::inconclusive(why),
        };
        if !exists {
            let observed = match self.kind {
                Kind::Sha256(_) => Value::Null,
                Kind::Exists | Kind::Contains(_) => json!(false),
            };
            return Observation::fail(observed, format!("{named} doesn't exist"));
        }
        match &self.kind {
            Kind::Exists => Observation::pass(json!(true), format!("{named} exists")),
            _ if self.path.is_dir() => {
                Observation::fail(Value::Null, format!("{named} is a directory, not a file"))
            }
            Kind::Contains(pattern) => self.search(pattern),
            Kind::Sha256(claimed) => self.hash(claimed),
        }
    }

    /// Whether the path exists, once the links along it are followed, as long as it stays under
    /// the root. A link under the root can lead out of it, or nowhere, so links are followed
    /// before anything is read or said about the file: what lies outside the root, even whether
    /// it exists, is never learned.
    fn resolve(&self) -> Result<bool, String> {
        let named = &self.named;
        let root = self
            .root
            .canonicalize()
            .map_err(|error| format!("can't resolve the file verifier's root: {error}"))?;
        // The deepest part of the path that is there, whether a file, a directory or a link.
        let mut there = self.path.as_path();
        loop {
            match there.symlink_metadata() {
                Ok(_) => break,
                Err(error)
                    if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
                {
                    there = there
                        .parent()
                        .ok_or_else(|| format!("can't look for {named}"))?;
                }
                Err(error) => return Err(format!("can't look for {named}: {error}")),
            }
        }
        let real = there
            .canonicalize()
            .map_err(|_| format!("{named} leads through a link to nothing"))?;
        if !real.starts_with(&root) {
            return Err(format!("{named} leads out of the file verifier's root"));
        }
        Ok(there == self.path)
    }

    fn search(&self, pattern: &str) -> Observation {
        let named = &self.named;
        if pattern.is_empty() {
            return Observation::inconclusive(NOTHING_TO_FIND);
        }
        match self.path.metadata() {
            Ok(metadata) if metadata.len() > MAX_SEARCHED => {
                return Observation::inconclusive(format!(
                    "{named} is larger than the {} MiB the file verifier searches",
                    MAX_SEARCHED / (1024 * 1024)
                ));
            }
            Ok(_) => {}
            Err(error) => return Observation::inconclusive(format!("can't read {named}: {error}")),
        }
        match std::fs::read(&self.path) {
            Ok(content) if contains(&content, pattern.as_bytes()) => {
                Observation::pass(json!(true), format!("{named} contains {pattern:?}"))
            }
            Ok(_) => {
                Observation::fail(json!(false), format!("{named} doesn't contain {pattern:?}"))
            }
            Err(error) => Observation::inconclusive(format!("can't read {named}: {error}")),
        }
    }

    fn hash(&self, claimed: &str) -> Observation {
        let named = &self.named;
        let hash = match sha256(&self.path) {
            Ok(hash) => hash,
            Err(error) => return Observation::inconclusive(format!("can't read {named}: {error}")),
        };
        let observed = format!("sha256:{hash}");
        let claimed = claimed.trim().to_ascii_lowercase();
        let summary = format!("{named} hashes to {observed}");
        if claimed.strip_prefix("sha256:").unwrap_or(&claimed) == hash {
            Observation::pass(json!(observed), summary)
        } else {
            Observation::fail(json!(observed), summary)
        }
    }
}

/// The file's SHA-256, as lowercase hex.
fn sha256(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        match file.read(&mut buffer)? {
            0 => break,
            read => hasher.update(&buffer[..read]),
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
