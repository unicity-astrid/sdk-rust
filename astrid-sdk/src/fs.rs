//! Virtual filesystem — mirrors [`std::fs`] naming and conventions.
//!
//! All paths are UTF-8 strings (the Astrid VFS has no concept of OS-specific
//! path encoding). Operations go through the host via FFI — the WASM guest
//! has no direct filesystem access.

use super::*;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Describes the type of a filesystem entry.
///
/// Mirrors [`std::fs::FileType`], restricted to the categories the Astrid
/// VFS supports (regular files and directories).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileType {
    is_dir: bool,
}

impl FileType {
    /// Returns `true` if this type represents a directory.
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Returns `true` if this type represents a regular file.
    pub fn is_file(&self) -> bool {
        !self.is_dir
    }
}

/// Metadata about a file or directory.
///
/// Mirrors [`std::fs::Metadata`] for the subset of fields the Astrid VFS
/// exposes. Returned by [`metadata`].
#[derive(Debug, Clone)]
pub struct Metadata {
    size: u64,
    is_dir: bool,
    mtime: u64,
}

impl Metadata {
    /// Returns the size in bytes. Mirrors [`std::fs::Metadata::len`].
    pub fn len(&self) -> u64 {
        self.size
    }

    /// Returns `true` if the size is zero.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns `true` if this metadata describes a directory.
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Returns `true` if this metadata describes a regular file.
    pub fn is_file(&self) -> bool {
        !self.is_dir
    }

    /// Returns the file type.
    pub fn file_type(&self) -> FileType {
        FileType {
            is_dir: self.is_dir,
        }
    }

    /// Returns the last modification time.
    ///
    /// The host reports modification time in seconds since the UNIX epoch.
    /// Returns `Err` if the timestamp is 0 (unavailable).
    pub fn modified(&self) -> Result<std::time::SystemTime, SysError> {
        if self.mtime == 0 {
            return Err(SysError::ApiError("modification time unavailable".into()));
        }
        Ok(std::time::UNIX_EPOCH + std::time::Duration::from_secs(self.mtime))
    }
}

/// A directory entry returned by [`read_dir`].
///
/// Mirrors [`std::fs::DirEntry`] for the fields the Astrid VFS provides.
/// The full path is constructed at iteration time from the parent directory
/// and entry name.
///
/// Note: `metadata()` and `file_type()` are not available on `DirEntry`
/// because the host resolves entries as names only. Use [`metadata`] with
/// the full path if you need per-entry metadata.
#[derive(Debug, Clone)]
pub struct DirEntry {
    path: String,
    name_offset: usize,
}

impl DirEntry {
    /// Returns the file name of this entry.
    ///
    /// Returns `&str` rather than `OsString` because VFS paths are always
    /// UTF-8.
    pub fn file_name(&self) -> &str {
        &self.path[self.name_offset..]
    }

    /// Returns the full path to this entry.
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Iterator over directory entries returned by [`read_dir`].
///
/// Unlike [`std::fs::ReadDir`], items are not wrapped in `Result` because
/// the host resolves all entries in a single call — per-entry failure is
/// not possible.
#[derive(Debug)]
pub struct ReadDir {
    entries: std::vec::IntoIter<DirEntry>,
}

impl Iterator for ReadDir {
    type Item = DirEntry;

    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// Check if a path exists. Like [`std::fs::exists`] (nightly).
pub fn exists(path: impl AsRef<[u8]>) -> Result<bool, SysError> {
    let result = unsafe { astrid_fs_exists(path.as_ref().to_vec())? };
    Ok(!result.is_empty() && result[0] != 0)
}

/// Read the entire contents of a file as bytes. Like [`std::fs::read`].
pub fn read(path: impl AsRef<[u8]>) -> Result<Vec<u8>, SysError> {
    let result = unsafe { astrid_read_file(path.as_ref().to_vec())? };
    Ok(result)
}

/// Read the entire contents of a file as a string. Like [`std::fs::read_to_string`].
pub fn read_to_string(path: impl AsRef<[u8]>) -> Result<String, SysError> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|e| SysError::ApiError(e.to_string()))
}

/// Write bytes to a file. Like [`std::fs::write`].
pub fn write(path: impl AsRef<[u8]>, contents: impl AsRef<[u8]>) -> Result<(), SysError> {
    unsafe { astrid_write_file(path.as_ref().to_vec(), contents.as_ref().to_vec())? };
    Ok(())
}

/// Create a directory. Like [`std::fs::create_dir`].
pub fn create_dir(path: impl AsRef<[u8]>) -> Result<(), SysError> {
    unsafe { astrid_fs_mkdir(path.as_ref().to_vec())? };
    Ok(())
}

/// Read directory entries. Like [`std::fs::read_dir`].
///
/// Returns an iterator over the entries in the directory. The host resolves
/// all entries in a single call, so the iterator is fully materialized.
pub fn read_dir(path: impl AsRef<[u8]>) -> Result<ReadDir, SysError> {
    let result = unsafe { astrid_fs_readdir(path.as_ref().to_vec())? };
    let path_str = String::from_utf8_lossy(path.as_ref());
    let parent = if path_str.ends_with('/') || path_str.is_empty() {
        path_str.into_owned()
    } else {
        format!("{path_str}/")
    };
    let names: Vec<String> = serde_json::from_slice(&result)?;
    let entries = names
        .into_iter()
        .map(|name| {
            let full_path = format!("{parent}{name}");
            let name_offset = parent.len();
            DirEntry {
                path: full_path,
                name_offset,
            }
        })
        .collect::<Vec<_>>();
    Ok(ReadDir {
        entries: entries.into_iter(),
    })
}

/// Get file metadata. Like [`std::fs::metadata`].
pub fn metadata(path: impl AsRef<[u8]>) -> Result<Metadata, SysError> {
    let result = unsafe { astrid_fs_stat(path.as_ref().to_vec())? };
    #[derive(Deserialize)]
    struct RawMetadata {
        size: u64,
        #[serde(rename = "isDir")]
        is_dir: bool,
        mtime: u64,
    }
    let raw: RawMetadata = serde_json::from_slice(&result)?;
    Ok(Metadata {
        size: raw.size,
        is_dir: raw.is_dir,
        mtime: raw.mtime,
    })
}

/// Remove a file. Like [`std::fs::remove_file`].
pub fn remove_file(path: impl AsRef<[u8]>) -> Result<(), SysError> {
    unsafe { astrid_fs_unlink(path.as_ref().to_vec())? };
    Ok(())
}
