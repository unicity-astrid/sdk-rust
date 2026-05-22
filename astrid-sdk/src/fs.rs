//! Virtual filesystem — mirrors [`std::fs`] naming and conventions.
//!
//! All paths are UTF-8 strings (the Astrid VFS has no concept of OS-specific
//! path encoding). Operations go through the host via WIT imports — the WASM
//! guest has no direct filesystem access.
//!
//! Open file handles are RAII via [`File`], which wraps the resource-backed
//! [`astrid_sys::astrid::fs::host::FileHandle`]; the kernel releases the
//! underlying descriptor on drop. Per-capsule cap: 16 open handles.

use super::*;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Describes the type of a filesystem entry. Mirrors [`std::fs::FileType`]
/// projected onto the categories the Astrid VFS surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    /// Regular file.
    Regular,
    /// Directory.
    Directory,
    /// Symbolic link (seen via [`symlink_metadata`]).
    Symlink,
    /// Block device, character device, FIFO, socket, or unknown — any
    /// non-regular, non-directory, non-symlink entry.
    Other,
}

impl FileType {
    fn from_wit(kind: wit_fs::FileType) -> Self {
        match kind {
            wit_fs::FileType::Regular => Self::Regular,
            wit_fs::FileType::Directory => Self::Directory,
            wit_fs::FileType::Symlink => Self::Symlink,
            _ => Self::Other,
        }
    }

    /// Returns `true` if this represents a regular file.
    #[must_use]
    pub fn is_file(self) -> bool {
        matches!(self, Self::Regular)
    }

    /// Returns `true` if this represents a directory.
    #[must_use]
    pub fn is_dir(self) -> bool {
        matches!(self, Self::Directory)
    }

    /// Returns `true` if this represents a symbolic link.
    #[must_use]
    pub fn is_symlink(self) -> bool {
        matches!(self, Self::Symlink)
    }
}

/// Metadata about a file or directory. Mirrors [`std::fs::Metadata`].
#[derive(Debug, Clone)]
pub struct Metadata {
    size: u64,
    kind: FileType,
    mode: u32,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    accessed: Option<std::time::SystemTime>,
}

impl Metadata {
    fn from_wit(stat: wit_fs::FileStat) -> Self {
        Self {
            size: stat.size,
            kind: FileType::from_wit(stat.kind),
            mode: stat.mode,
            modified: stat.modified.and_then(datetime_to_system_time),
            created: stat.created.and_then(datetime_to_system_time),
            accessed: stat.accessed.and_then(datetime_to_system_time),
        }
    }

    /// Size in bytes. Mirrors [`std::fs::Metadata::len`].
    #[must_use]
    pub fn len(&self) -> u64 {
        self.size
    }

    /// `true` if the size is zero.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// `true` if this metadata describes a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.kind.is_dir()
    }

    /// `true` if this metadata describes a regular file.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.kind.is_file()
    }

    /// `true` if this metadata describes a symbolic link
    /// (only via [`symlink_metadata`]; following stat resolves links).
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        self.kind.is_symlink()
    }

    /// File type.
    #[must_use]
    pub fn file_type(&self) -> FileType {
        self.kind
    }

    /// POSIX mode bits (file type + permissions). Zero on platforms
    /// without a mode concept; best-effort cross-platform.
    #[must_use]
    pub fn mode(&self) -> u32 {
        self.mode
    }

    /// Last modification time, if the host reported one.
    pub fn modified(&self) -> Result<std::time::SystemTime, SysError> {
        self.modified
            .ok_or_else(|| SysError::ApiError("modification time unavailable".into()))
    }

    /// Creation (birth) time, if the host reported one.
    pub fn created(&self) -> Result<std::time::SystemTime, SysError> {
        self.created
            .ok_or_else(|| SysError::ApiError("creation time unavailable".into()))
    }

    /// Last access time, if the host reported one.
    pub fn accessed(&self) -> Result<std::time::SystemTime, SysError> {
        self.accessed
            .ok_or_else(|| SysError::ApiError("access time unavailable".into()))
    }
}

fn datetime_to_system_time(dt: wit_fs::Datetime) -> Option<std::time::SystemTime> {
    // The contract uses signed seconds for archive-restore use cases; we
    // only support post-epoch times here (matching `std::time::SystemTime`).
    if dt.seconds < 0 {
        return None;
    }
    let secs = u64::try_from(dt.seconds).ok()?;
    let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    Some(base + std::time::Duration::from_nanos(u64::from(dt.nanoseconds)))
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
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.path[self.name_offset..]
    }

    /// Returns the full path to this entry.
    #[must_use]
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

/// Open-options analogue. Mirrors the common subset of
/// [`std::fs::OpenOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// Open existing file for reading only. Fails if absent.
    Read,
    /// Open or create for writing; truncates existing content.
    Write,
    /// Open or create for writing; appends to existing content.
    Append,
    /// Open existing file for both reading and writing. Fails if absent.
    ReadWrite,
}

impl OpenMode {
    fn to_wit(self) -> wit_fs::OpenMode {
        match self {
            Self::Read => wit_fs::OpenMode::Read,
            Self::Write => wit_fs::OpenMode::Write,
            Self::Append => wit_fs::OpenMode::Append,
            Self::ReadWrite => wit_fs::OpenMode::ReadWrite,
        }
    }
}

/// An open file handle. RAII over the kernel-side
/// [`astrid_sys::astrid::fs::host::FileHandle`] resource — drop
/// releases the descriptor automatically.
///
/// Reads and writes are positional ([`File::read_at`] / [`File::write_at`]);
/// there is no implicit cursor.
#[derive(Debug)]
pub struct File {
    inner: wit_fs::FileHandle,
}

impl File {
    /// Open an existing file for reading.
    pub fn open(path: &str) -> Result<Self, SysError> {
        let inner = wit_fs::fs_open(path, wit_fs::OpenMode::Read).map_err(host_err)?;
        Ok(Self { inner })
    }

    /// Open or create a file for writing; truncates existing content.
    pub fn create(path: &str) -> Result<Self, SysError> {
        let inner = wit_fs::fs_open(path, wit_fs::OpenMode::Write).map_err(host_err)?;
        Ok(Self { inner })
    }

    /// Open with a specific [`OpenMode`].
    pub fn open_mode(path: &str, mode: OpenMode) -> Result<Self, SysError> {
        let inner = wit_fs::fs_open(path, mode.to_wit()).map_err(host_err)?;
        Ok(Self { inner })
    }

    /// Positional read up to `max_bytes` from byte `offset` (POSIX
    /// `pread(2)`). Empty result signals EOF at the requested offset.
    /// Per-call payload capped at 1 MB.
    pub fn read_at(&self, offset: u64, max_bytes: u32) -> Result<Vec<u8>, SysError> {
        self.inner.read_at(offset, max_bytes).map_err(host_err)
    }

    /// Positional write at byte `offset` (POSIX `pwrite(2)`). Returns
    /// the number of bytes written. Per-call payload capped at 1 MB.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> Result<u32, SysError> {
        self.inner.write_at(offset, data).map_err(host_err)
    }

    /// Flush buffered data to disk (`fdatasync(2)` — data only).
    pub fn sync_data(&self) -> Result<(), SysError> {
        self.inner.sync_data().map_err(host_err)
    }

    /// Flush both data and metadata to disk (`fsync(2)`).
    pub fn sync_all(&self) -> Result<(), SysError> {
        self.inner.sync_all().map_err(host_err)
    }

    /// Get metadata for this open file (`fstat(2)`).
    pub fn metadata(&self) -> Result<Metadata, SysError> {
        let stat = self.inner.stat().map_err(host_err)?;
        Ok(Metadata::from_wit(stat))
    }

    /// Truncate or extend the file to `size` bytes (`ftruncate(2)`).
    pub fn set_len(&self, size: u64) -> Result<(), SysError> {
        self.inner.set_len(size).map_err(host_err)
    }
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// Check if a path exists. Like [`std::fs::exists`] (nightly).
pub fn exists(path: &str) -> Result<bool, SysError> {
    wit_fs::fs_exists(path).map_err(host_err)
}

/// Read the entire contents of a file as bytes. Like [`std::fs::read`].
///
/// Files larger than 10 MB are rejected (`too-large`); use [`File::open`]
/// + [`File::read_at`] for streaming.
pub fn read(path: &str) -> Result<Vec<u8>, SysError> {
    wit_fs::read_file(path).map_err(host_err)
}

/// Read the entire contents of a file as a UTF-8 string.
/// Like [`std::fs::read_to_string`].
pub fn read_to_string(path: &str) -> Result<String, SysError> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|e| SysError::ApiError(e.to_string()))
}

/// Write bytes to a file (truncate-or-create). Like [`std::fs::write`].
/// Capped at 10 MB per call.
pub fn write(path: &str, contents: &[u8]) -> Result<(), SysError> {
    wit_fs::write_file(path, contents).map_err(host_err)
}

/// Append `contents` to a file, creating it if absent.
///
/// Avoids the read-modify-write race of [`read`] + [`write`] for
/// log-style writers. Capped at 10 MB per call.
pub fn append(path: &str, contents: &[u8]) -> Result<(), SysError> {
    wit_fs::fs_append(path, contents).map_err(host_err)
}

/// Create a directory. Like [`std::fs::create_dir`] — strict (fails if
/// the path exists or the parent is missing).
pub fn create_dir(path: &str) -> Result<(), SysError> {
    wit_fs::fs_mkdir(path).map_err(host_err)
}

/// Create a directory and all missing parents. Like
/// [`std::fs::create_dir_all`] — idempotent.
pub fn create_dir_all(path: &str) -> Result<(), SysError> {
    wit_fs::fs_mkdir_all(path).map_err(host_err)
}

/// Copy a file from `src` to `dst`. Like [`std::fs::copy`]. Overwrites
/// `dst`. Directory copies are not supported.
pub fn copy(src: &str, dst: &str) -> Result<(), SysError> {
    wit_fs::fs_copy(src, dst).map_err(host_err)
}

/// Rename (move) within the same VFS scheme. Like [`std::fs::rename`].
/// Cross-scheme renames return `cross-vfs`.
pub fn rename(src: &str, dst: &str) -> Result<(), SysError> {
    wit_fs::fs_rename(src, dst).map_err(host_err)
}

/// Remove a directory and all its contents recursively.
///
/// Refuses to traverse symlinks to prevent sandbox escapes. Returns
/// the number of entries removed.
pub fn remove_dir_all(path: &str) -> Result<u64, SysError> {
    wit_fs::fs_remove_dir_all(path).map_err(host_err)
}

/// Canonicalize a path. Like [`std::fs::canonicalize`].
///
/// Returns a VFS-scheme path. NOT a security check that subsequent
/// calls can trust — the kernel re-resolves every path on every call.
pub fn canonicalize(path: &str) -> Result<String, SysError> {
    wit_fs::fs_canonicalize(path).map_err(host_err)
}

/// Read the target of a symbolic link without following it.
/// Like [`std::fs::read_link`].
pub fn read_link(path: &str) -> Result<String, SysError> {
    wit_fs::fs_read_link(path).map_err(host_err)
}

/// Create a hard link from `link_path` to `src`. Like
/// [`std::fs::hard_link`]. Both paths must resolve to the same VFS
/// scheme; cross-scheme links return `cross-vfs`.
pub fn hard_link(src: &str, link_path: &str) -> Result<(), SysError> {
    wit_fs::fs_hard_link(src, link_path).map_err(host_err)
}

/// Read directory entries. Like [`std::fs::read_dir`].
///
/// Returns an iterator over the entries in the directory. The host resolves
/// all entries in a single call, so the iterator is fully materialized.
/// Per-call cap: 4096 entries.
pub fn read_dir(path: &str) -> Result<ReadDir, SysError> {
    let names = wit_fs::fs_readdir(path).map_err(host_err)?;
    let parent = if path.ends_with('/') || path.is_empty() {
        path.to_string()
    } else {
        format!("{path}/")
    };
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

/// Get file metadata, following symlinks. Like [`std::fs::metadata`].
pub fn metadata(path: &str) -> Result<Metadata, SysError> {
    let stat = wit_fs::fs_stat(path).map_err(host_err)?;
    Ok(Metadata::from_wit(stat))
}

/// Get file metadata without following symlinks (`lstat(2)`). Like
/// [`std::fs::symlink_metadata`].
pub fn symlink_metadata(path: &str) -> Result<Metadata, SysError> {
    let stat = wit_fs::fs_stat_symlink(path).map_err(host_err)?;
    Ok(Metadata::from_wit(stat))
}

/// Remove a file. Like [`std::fs::remove_file`].
pub fn remove_file(path: &str) -> Result<(), SysError> {
    wit_fs::fs_unlink(path).map_err(host_err)
}
