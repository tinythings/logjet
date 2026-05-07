use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::dataset_index::{DatasetIndex, load_or_build};
use crate::error::{Error, Result};

/// Stable dataset manifest for multi-file `.logjet` scans.
#[derive(Debug, Clone)]
pub(crate) struct Dataset {
    entries: Vec<DatasetEntry>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DatasetOptions {
    pub(crate) load_index: bool,
}

impl DatasetOptions {
    pub(crate) fn default() -> Self {
        Self { load_index: true }
    }

    pub(crate) fn nfs() -> Self {
        Self { load_index: false }
    }
}

/// Cheap per-file manifest metadata gathered without opening the payload stream.
#[derive(Debug, Clone)]
pub(crate) struct DatasetEntry {
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    pub(crate) modified_ns: Option<u64>,
    pub(crate) first_seq: Option<u64>,
    pub(crate) last_seq: Option<u64>,
    pub(crate) first_ts_unix_ns: Option<u64>,
    pub(crate) last_ts_unix_ns: Option<u64>,
    pub(crate) index: Option<DatasetIndex>,
}

impl Dataset {
    pub(crate) fn from_inputs(inputs: &[PathBuf]) -> Result<Self> {
        Self::from_inputs_with_options(inputs, DatasetOptions::default())
    }

    pub(crate) fn from_inputs_with_options(inputs: &[PathBuf], options: DatasetOptions) -> Result<Self> {
        if inputs.is_empty() {
            return Err(Error::Usage("dataset selection is empty; pass one or more .logjet files".to_string()));
        }
        if inputs.iter().any(|path| path == Path::new("-")) {
            if inputs.len() == 1 {
                return Ok(Self {
                    entries: vec![DatasetEntry {
                        path: PathBuf::from("-"),
                        size: 0,
                        modified_ns: None,
                        first_seq: None,
                        last_seq: None,
                        first_ts_unix_ns: None,
                        last_ts_unix_ns: None,
                        index: None,
                    }],
                });
            }
            return Err(Error::Usage("stdin cannot be mixed with file inputs in `ljx view`".to_string()));
        }

        let mut paths = Vec::new();
        for input in inputs {
            if !input.exists() {
                let note = if looks_like_glob(input) { "; `ljx view` expects the shell to expand globs" } else { "" };
                return Err(Error::Usage(format!("input {} does not exist{note}", input.display())));
            }
            let meta = std::fs::metadata(input)?;
            if meta.is_dir() {
                collect_dir_entries(input, &mut paths)?;
                continue;
            }
            if !meta.is_file() {
                return Err(Error::Usage(format!("input {} is not a regular file", input.display())));
            }
            paths.push(input.clone());
        }

        if paths.is_empty() {
            return Err(Error::Usage("dataset selection resolved to no .logjet files".to_string()));
        }

        paths.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
        paths.dedup();
        Ok(Self { entries: paths.into_iter().map(|path| DatasetEntry::from_path(path, options)).collect::<Result<Vec<_>>>()? })
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_stdin(&self) -> bool {
        self.len() == 1 && self.entries[0].path == Path::new("-")
    }

    pub(crate) fn primary_path(&self) -> &Path {
        &self.entries[0].path
    }

    pub(crate) fn entries(&self) -> &[DatasetEntry] {
        &self.entries
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.entries.iter().map(|entry| entry.path.as_path())
    }

    pub(crate) fn output_dir(&self) -> PathBuf {
        let mut parents = self.paths().filter(|path| *path != Path::new("-")).filter_map(|path| path.parent().map(Path::to_path_buf));
        let Some(first) = parents.next() else {
            return PathBuf::from(".");
        };
        if parents.all(|path| path == first) { first } else { PathBuf::from(".") }
    }

    pub(crate) fn default_stem(&self, fallback: &str) -> String {
        if self.len() == 1 { self.primary_path().file_stem().and_then(|s| s.to_str()).unwrap_or(fallback).to_string() } else { "dataset".to_string() }
    }
}

impl DatasetEntry {
    fn from_path(path: PathBuf, options: DatasetOptions) -> Result<Self> {
        let meta = std::fs::metadata(&path)?;
        let size = meta.len();
        let modified_ns = modified_ns(&meta);
        let index = if options.load_index { load_or_build(&path, size, modified_ns) } else { None };
        Ok(Self {
            path,
            size,
            modified_ns,
            first_seq: index.as_ref().and_then(|idx| idx.summary.first_seq),
            last_seq: index.as_ref().and_then(|idx| idx.summary.last_seq),
            first_ts_unix_ns: index.as_ref().and_then(|idx| idx.summary.first_ts_unix_ns),
            last_ts_unix_ns: index.as_ref().and_then(|idx| idx.summary.last_ts_unix_ns),
            index,
        })
    }
}

fn collect_dir_entries(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut found = 0usize;
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(current) = dirs.pop() {
        for entry in std::fs::read_dir(&current)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                dirs.push(entry.path());
                continue;
            }
            if !ty.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("logjet") {
                continue;
            }
            out.push(path);
            found += 1;
        }
    }
    if found == 0 {
        return Err(Error::Usage(format!("input {} is a directory with no .logjet files", dir.display())));
    }
    Ok(())
}

fn looks_like_glob(path: &Path) -> bool {
    path.as_os_str().to_string_lossy().bytes().any(|b| matches!(b, b'*' | b'?' | b'['))
}

fn modified_ns(meta: &Metadata) -> Option<u64> {
    meta.modified().ok().and_then(|ts| ts.duration_since(UNIX_EPOCH).ok()).and_then(|dur| u64::try_from(dur.as_nanos()).ok())
}

#[cfg(test)]
#[path = "../tests/unit/dataset_ut.rs"]
mod dataset_ut;
