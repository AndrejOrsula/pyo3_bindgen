//! Canonical formatting, comparison, atomic publication, and durability reporting.
//!
//! [`TextArtifact`] and [`RustArtifact`] expose direct, single-artifact operations. Their writers
//! provide regular-file/symbolic-link safeguards and staged atomic replacement. The detailed
//! writer exposes the platform durability result; ordinary `write` reports a Unix
//! parent-directory synchronization failure after replacement but treats `PlatformUnsupported`
//! as success. The direct APIs intentionally do not provide the command-facing Windows policy
//! for namespaces, reserved DOS device names, drive-relative paths, or trailing-dot/space
//! components. Generic checks such as a missing final name and embedded NUL code units still
//! apply. Callers that require the stricter policy should use the CLI or `Codegen::build`
//! publication path.

mod value;

use std::fs::OpenOptions;
use std::{
    fmt,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    static INJECT_PARENT_DIRECTORY_SYNC_FAILURE: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

// Win32 `ReplaceFileW` partial-transition errors. Keep the documented numeric
// values available to host tests even when `windows-sys` is not a host dependency.
#[cfg(any(test, windows))]
const ERROR_UNABLE_TO_MOVE_REPLACEMENT: i32 = 1176;
#[cfg(any(test, windows))]
const ERROR_UNABLE_TO_MOVE_REPLACEMENT_2: i32 = 1177;

/// Durability status for an atomically published artifact.
#[doc(hidden)]
#[derive(Debug)]
pub enum ArtifactDurability {
    /// The destination replacement and parent-directory synchronization succeeded.
    DirectorySynchronized,
    /// This platform has no supported parent-directory synchronization operation.
    PlatformUnsupported,
    /// The destination replacement succeeded, but parent-directory synchronization failed.
    DirectorySyncFailed(io::Error),
}

/// Receipt returned after an artifact has been atomically published.
#[doc(hidden)]
#[derive(Debug)]
pub struct ArtifactWriteReceipt {
    /// Durability status for the published destination.
    pub durability: ArtifactDurability,
}

/// Typed failure from detailed artifact publication.
#[doc(hidden)]
#[derive(Debug)]
pub enum ArtifactWriteFailure {
    /// The destination was not published.
    NotPublished(io::Error),
    /// Publication may have happened; the staged file is retained for recovery.
    PossiblyPublished {
        /// The replacement error, including recovery context.
        error: io::Error,
        /// Path of the retained staged replacement.
        staged_path: PathBuf,
    },
}

impl ArtifactWriteFailure {
    fn into_io_error(self) -> io::Error {
        match self {
            Self::NotPublished(error) | Self::PossiblyPublished { error, .. } => error,
        }
    }
}

impl From<io::Error> for ArtifactWriteFailure {
    fn from(error: io::Error) -> Self {
        Self::NotPublished(error)
    }
}

impl fmt::Display for ArtifactWriteFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPublished(error) | Self::PossiblyPublished { error, .. } => {
                error.fmt(formatter)
            }
        }
    }
}

impl std::error::Error for ArtifactWriteFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotPublished(error) | Self::PossiblyPublished { error, .. } => Some(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StagedFileState {
    Unchanged,
    #[cfg(any(test, windows))]
    ReplacementPreserved(PartialReplaceState),
}

#[cfg(any(test, windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartialReplaceState {
    DestinationMayBeAbsent,
    DestinationMovedAndMetadataInherited,
}

#[derive(Debug)]
struct ReplaceFailure {
    error: io::Error,
    staged_file: StagedFileState,
}

impl ReplaceFailure {
    fn unchanged(error: io::Error) -> Self {
        Self {
            error,
            staged_file: StagedFileState::Unchanged,
        }
    }

    #[cfg(any(test, windows))]
    fn from_windows_error(error: io::Error) -> Self {
        let staged_file = match error.raw_os_error() {
            Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT) => {
                StagedFileState::ReplacementPreserved(PartialReplaceState::DestinationMayBeAbsent)
            }
            Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2) => StagedFileState::ReplacementPreserved(
                PartialReplaceState::DestinationMovedAndMetadataInherited,
            ),
            _ => StagedFileState::Unchanged,
        };
        Self { error, staged_file }
    }

    fn into_failure(self, staged: &Path) -> ArtifactWriteFailure {
        match self.staged_file {
            StagedFileState::Unchanged => {
                let _ = std::fs::remove_file(staged);
                ArtifactWriteFailure::NotPublished(self.error)
            }
            #[cfg(any(test, windows))]
            StagedFileState::ReplacementPreserved(partial_state) => {
                ArtifactWriteFailure::PossiblyPublished {
                    error: partial_replacement_error(self.error, staged, partial_state),
                    staged_path: staged.to_path_buf(),
                }
            }
        }
    }

    #[cfg(test)]
    fn finish(self, staged: &Path) -> io::Error {
        self.into_failure(staged).into_io_error()
    }
}

#[cfg(any(test, windows))]
fn partial_replacement_error(
    error: io::Error,
    staged: &Path,
    partial_state: PartialReplaceState,
) -> io::Error {
    let documented_state = match partial_state {
        PartialReplaceState::DestinationMayBeAbsent => "destination may be absent",
        PartialReplaceState::DestinationMovedAndMetadataInherited => {
            "original destination may have moved to another name; staged replacement may have inherited metadata"
        }
    };
    io::Error::new(
        error.kind(),
        format!(
            "{}; replacement may be partial; staged replacement preserved at {}; {documented_state}",
            error,
            staged.display()
        ),
    )
}

/// Comparison result for an artifact and an existing file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactStatus {
    /// The existing file is byte-identical to the artifact.
    Current,
    /// The existing file differs from the artifact.
    Drifted,
}

/// An immutable UTF-8 artifact with shared hashing, checking, and publication behavior.
///
/// Direct artifact operations are single-file operations. They enforce regular-file and
/// symbolic-link safeguards and stage replacements atomically. They omit command-facing Windows
/// policy for namespaces, reserved-device names, drive-relative paths, and trailing-dot/space
/// components; generic checks such as a missing final name and embedded NUL code units still
/// apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextArtifact {
    contents: String,
}

impl TextArtifact {
    /// Compare the artifact to an existing regular file.
    ///
    /// Symbolic links and other non-regular paths are rejected. The opened handle is
    /// revalidated before reading; Unix additionally uses no-follow and nonblocking
    /// open flags so a concurrent path swap cannot redirect or block the check. This direct
    /// operation does not apply the command-facing Windows policy for namespaces, reserved-device
    /// names, drive-relative paths, or trailing-dot/space components; generic path checks still
    /// apply.
    pub fn check(&self, path: impl AsRef<Path>) -> io::Result<ArtifactStatus> {
        let path = path.as_ref();
        let mut file = open_regular_file_for_read(path)?;
        let mut existing = String::new();
        file.read_to_string(&mut existing)?;
        Ok(if existing == self.contents {
            ArtifactStatus::Current
        } else {
            ArtifactStatus::Drifted
        })
    }

    /// Atomically replace a file with this artifact on success.
    ///
    /// The file is staged and synchronized in the destination directory before
    /// replacement. Existing destination permissions are preserved; symbolic links
    /// and other non-regular destinations are rejected. New files are published with a
    /// private mode on Unix; callers can pre-create a file when another final
    /// mode is required. Windows preserves an existing file's ACL and attributes;
    /// its ACL-preserving replacement API has no supported write-through mode,
    /// so only the staged file can be explicitly synchronized there. If that API
    /// reports a documented partial move, the staged replacement is preserved and
    /// its recovery path is included in the returned error. On Unix, a parent
    /// directory synchronization failure is returned after the replacement has
    /// already occurred; use [`Self::write_detailed`] to distinguish that
    /// durability outcome from pre-publication failures.
    ///
    /// This direct writer intentionally does not apply the command-facing Windows policy for
    /// namespaces, reserved DOS device names, drive-relative paths, or trailing-dot/space
    /// components. Generic checks such as a missing final name and embedded NUL code units still
    /// apply. Use the CLI or `Codegen::build` when that stricter policy is required.
    pub fn write(&self, path: impl AsRef<Path>) -> io::Result<()> {
        write_atomically(path.as_ref(), self.bytes())
    }

    /// Publish this artifact and expose whether the parent directory was synchronized.
    ///
    /// Unlike [`Self::write`], this detailed result preserves the `PlatformUnsupported` and
    /// `DirectorySyncFailed` durability distinctions. Its direct writer has the same intentionally
    /// narrow Windows path boundary as [`Self::write`].
    #[doc(hidden)]
    pub fn write_detailed(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ArtifactWriteReceipt, ArtifactWriteFailure> {
        write_atomically_detailed(path.as_ref(), self.bytes())
    }
}

fn reject_non_regular_path(path: &Path, metadata: &std::fs::Metadata) -> io::Result<()> {
    if metadata.file_type().is_symlink() {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "artifact path {} must not be a symbolic link destination",
                path.display()
            ),
        ))
    } else if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "artifact path {} exists but is not a regular file",
                path.display()
            ),
        ))
    }
}

#[cfg(unix)]
fn open_regular_file_for_read(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    reject_non_regular_path(path, &std::fs::symlink_metadata(path)?)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    reject_non_regular_path(path, &file.metadata()?)?;
    Ok(file)
}

#[cfg(windows)]
fn open_regular_file_for_read(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    reject_non_regular_path(path, &std::fs::symlink_metadata(path)?)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    reject_non_regular_path(path, &file.metadata()?)?;
    Ok(file)
}

#[cfg(all(not(unix), not(windows)))]
fn open_regular_file_for_read(path: &Path) -> io::Result<File> {
    reject_non_regular_path(path, &std::fs::symlink_metadata(path)?)?;
    let file = File::open(path)?;
    reject_non_regular_path(path, &file.metadata()?)?;
    Ok(file)
}

/// Canonically formatted Rust source generated from a token stream.
///
/// Its direct `check` and `write` methods inherit [`TextArtifact`]'s single-file safeguards. On
/// Windows, they omit command-facing policy for namespaces, reserved-device names, drive-relative
/// paths, and trailing-dot/space components; generic checks such as a missing final name and
/// embedded NUL code units still apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustArtifact {
    text: TextArtifact,
}

impl RustArtifact {
    /// Compare the formatted source to an existing file.
    ///
    /// This direct operation does not apply command-facing Windows policy for namespaces,
    /// reserved-device names, drive-relative paths, or trailing-dot/space components; generic
    /// path checks still apply.
    pub fn check(&self, path: impl AsRef<Path>) -> io::Result<ArtifactStatus> {
        self.text.check(path)
    }

    /// Atomically replace a file with the formatted source.
    ///
    /// This direct writer provides per-file atomic replacement and special-file safeguards, but
    /// does not apply command-facing Windows policy for namespaces, reserved-device names,
    /// drive-relative paths, or trailing-dot/space components. Generic checks such as a missing
    /// final name and embedded NUL code units still apply; use the CLI or `Codegen::build` for the
    /// stricter policy.
    pub fn write(&self, path: impl AsRef<Path>) -> io::Result<()> {
        self.text.write(path)
    }

    /// Publish this artifact and expose whether the parent directory was synchronized.
    ///
    /// Unlike [`Self::write`], this detailed result preserves the `PlatformUnsupported` and
    /// `DirectorySyncFailed` durability distinctions. Its direct writer has the same intentionally
    /// narrow Windows path boundary as [`Self::write`].
    #[doc(hidden)]
    pub fn write_detailed(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ArtifactWriteReceipt, ArtifactWriteFailure> {
        self.text.write_detailed(path)
    }
}

fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    match write_atomically_detailed(path, contents) {
        Ok(ArtifactWriteReceipt {
            durability: ArtifactDurability::DirectorySyncFailed(error),
        }) => Err(io::Error::new(
            error.kind(),
            format!(
                "artifact replacement already occurred, but parent directory synchronization failed after replacement: {error}"
            ),
        )),
        Ok(_) => Ok(()),
        Err(failure) => Err(failure.into_io_error()),
    }
}

fn write_atomically_detailed(
    path: &Path,
    contents: &[u8],
) -> Result<ArtifactWriteReceipt, ArtifactWriteFailure> {
    let span = tracing::info_span!(
        "pyo3_bindgen.artifact.write",
        path = %path.display(),
        bytes = contents.len(),
    );
    let _entered = span.enter();
    path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("artifact path {} has no file name", path.display()),
        )
    })?;
    let destination_permissions = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ArtifactWriteFailure::NotPublished(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "artifact path {} must not be a symbolic link destination",
                    path.display()
                ),
            )));
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(ArtifactWriteFailure::NotPublished(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "artifact path {} exists but is not a regular file",
                    path.display()
                ),
            )));
        }
        Ok(metadata) => Some(metadata.permissions()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let destination_exists = destination_permissions.is_some();
    let (temp_path, mut temp_file) = loop {
        let nonce = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let created_unix_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        // Keep the staging component independent of the destination component's
        // length. PID, timestamp, and monotonic nonce make concurrent/process-reuse
        // collisions improbable; `create_new` remains the authoritative guard.
        let temp_path = parent.join(format!(
            ".pyo3-bindgen.{:08x}.{created_unix_nanos:032x}.{nonce:016x}.tmp",
            std::process::id(),
        ));
        match create_staging_file(&temp_path, destination_exists.then_some(path)) {
            Ok(temp_file) => break (temp_path, temp_file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    };

    let staged = (|| {
        temp_file.write_all(contents)?;
        temp_file.sync_all()?;
        #[cfg(not(windows))]
        if let Some(permissions) = destination_permissions.as_ref() {
            // Apply the final mode only after all bytes are staged. This keeps
            // partial contents private and avoids writes clearing special bits.
            std::fs::set_permissions(&temp_path, permissions.clone())?;
            temp_file.sync_all()?;
        }
        Ok::<(), io::Error>(())
    })();
    if let Err(err) = staged {
        drop(temp_file);
        let _ = std::fs::remove_file(&temp_path);
        return Err(err.into());
    }
    drop(temp_file);

    if let Err(failure) = replace_file(&temp_path, path, destination_exists) {
        return Err(failure.into_failure(&temp_path));
    }
    let durability = parent_directory_durability(parent);
    if let ArtifactDurability::DirectorySyncFailed(ref err) = durability {
        tracing::warn!(
            parent = %parent.display(),
            path = %path.display(),
            error = %err,
            "parent directory metadata sync failed after visible artifact replacement"
        );
    }
    tracing::info!("artifact published");
    Ok(ArtifactWriteReceipt { durability })
}

#[cfg(unix)]
fn create_staging_file(path: &Path, _destination: Option<&Path>) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(all(not(unix), not(windows)))]
fn create_staging_file(path: &Path, _destination: Option<&Path>) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(windows)]
fn create_staging_file(path: &Path, destination: Option<&Path>) -> io::Result<File> {
    use std::{mem::size_of, os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL},
    };

    let mut security_descriptor = destination.map(windows_dacl).transpose()?.flatten();
    let mut security_attributes =
        security_descriptor
            .as_mut()
            .map(|descriptor| SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.as_mut_ptr().cast(),
                bInheritHandle: 0,
            });
    let security_attributes = security_attributes
        .as_mut()
        .map_or(ptr::null(), |attributes| attributes as *const _);
    let path = windows_path(path)?;
    let handle = unsafe {
        // SAFETY: `windows_path` rejects interior NULs and appends the required
        // terminator; the path and optional security descriptor stay alive for
        // this synchronous call. `security_attributes` is either null or points
        // to an initialized structure whose descriptor buffer remains owned by
        // `security_descriptor`. The returned value is checked against
        // `INVALID_HANDLE_VALUE` below.
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            0,
            security_attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe {
            // SAFETY: `handle` came from the successful `CreateFileW` call
            // above, so it is an owned file handle and not
            // `INVALID_HANDLE_VALUE`. `File` takes the sole responsibility for
            // closing it when dropped; no other owner retains this handle.
            File::from_raw_handle(handle.cast())
        })
    }
}

#[cfg(windows)]
fn windows_dacl(path: &Path) -> io::Result<Option<Vec<usize>>> {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::{
        Foundation::{ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED},
        Security::{GetFileSecurityW, DACL_SECURITY_INFORMATION},
    };

    let path = windows_path(path)?;
    let mut bytes_needed = 0;
    unsafe {
        // SAFETY: `path` is a live, NUL-terminated UTF-16 buffer. Passing a
        // null descriptor pointer with length zero is the documented size
        // probe, and `bytes_needed` is a valid writable DWORD for its output.
        // The probe's result is handled through `bytes_needed` and
        // `GetLastError` below.
        GetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            0,
            &mut bytes_needed,
        )
    };
    let size = size_of::<usize>();
    if bytes_needed == 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(code)
                if code == ERROR_INVALID_FUNCTION as i32 || code == ERROR_NOT_SUPPORTED as i32 =>
            {
                Ok(None)
            }
            _ => Err(error),
        };
    }

    let mut descriptor = vec![0usize; (bytes_needed as usize).div_ceil(size)];
    let loaded = unsafe {
        // SAFETY: `descriptor` has an allocation of at least `bytes_needed`
        // bytes and is properly aligned for the API's byte buffer; its pointer,
        // `path`, and the writable `bytes_needed` output remain valid for this
        // synchronous call. The BOOL result is checked before the descriptor
        // is returned, and `Vec` owns cleanup of the buffer.
        GetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            descriptor.as_mut_ptr().cast(),
            bytes_needed,
            &mut bytes_needed,
        )
    };
    if loaded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(Some(descriptor))
    }
}

#[cfg(not(windows))]
fn replace_file(
    staged: &Path,
    destination: &Path,
    _destination_exists: bool,
) -> Result<(), ReplaceFailure> {
    std::fs::rename(staged, destination).map_err(ReplaceFailure::unchanged)
}

#[cfg(windows)]
fn replace_file(
    staged: &Path,
    destination: &Path,
    destination_exists: bool,
) -> Result<(), ReplaceFailure> {
    use std::ptr;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH,
    };

    let staged = windows_path(staged).map_err(ReplaceFailure::unchanged)?;
    let destination = windows_path(destination).map_err(ReplaceFailure::unchanged)?;
    let replaced = unsafe {
        // SAFETY: both path vectors are live, NUL-terminated UTF-16 buffers for
        // the duration of the synchronous call. The null arguments to
        // `ReplaceFileW` are its documented optional/reserved parameters; the
        // `MoveFileExW` destination is non-null. Both BOOL results are handled
        // below before interpreting `GetLastError`.
        if destination_exists {
            // ReplaceFileW preserves ACLs and other destination attributes, but
            // its documented WRITE_THROUGH flag is unsupported. The staged file
            // has already been synchronized above.
            ReplaceFileW(
                destination.as_ptr(),
                staged.as_ptr(),
                ptr::null(),
                0,
                ptr::null(),
                ptr::null(),
            )
        } else {
            MoveFileExW(
                staged.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    if replaced == 0 {
        let error = io::Error::last_os_error();
        Err(if destination_exists {
            ReplaceFailure::from_windows_error(error)
        } else {
            ReplaceFailure::unchanged(error)
        })
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn windows_path(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if encoded.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows artifact paths must not contain NUL code units",
        ));
    }
    encoded.push(0);
    Ok(encoded)
}

fn parent_directory_durability(parent: &Path) -> ArtifactDurability {
    #[cfg(unix)]
    {
        match sync_parent_directory(parent) {
            Ok(()) => ArtifactDurability::DirectorySynchronized,
            Err(error) => ArtifactDurability::DirectorySyncFailed(error),
        }
    }

    #[cfg(not(unix))]
    {
        let _ = parent;
        ArtifactDurability::PlatformUnsupported
    }
}

// Only the Unix parent-directory sync path can have its failure injected, and the tests that
// do so are themselves Unix-only.
#[cfg(all(test, unix))]
fn inject_parent_directory_sync_failure() {
    INJECT_PARENT_DIRECTORY_SYNC_FAILURE.with(|failure| failure.set(true));
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    #[cfg(test)]
    if INJECT_PARENT_DIRECTORY_SYNC_FAILURE.with(|failure| {
        let injected = failure.get();
        failure.set(false);
        injected
    }) {
        return Err(io::Error::other("injected parent directory sync failure"));
    }

    std::fs::File::open(parent)?.sync_all()
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::windows_path;
    use std::{ffi::OsString, io, os::windows::ffi::OsStringExt, path::PathBuf};

    #[test]
    fn windows_path_rejects_an_interior_nul() {
        let path = PathBuf::from(OsString::from_wide(&[
            b'b' as u16,
            b'e' as u16,
            0,
            b'y' as u16,
            b'o' as u16,
            b'n' as u16,
            b'd' as u16,
        ]));

        let error = windows_path(&path).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}

#[cfg(test)]
mod replace_failure_tests {
    use super::{ArtifactWriteFailure, ReplaceFailure};
    use std::{io, path::PathBuf};

    fn staged_file(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "pyo3_bindgen_replace_failure_{name}_{}_{}",
            std::process::id(),
            super::TEMP_FILE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let staged = directory.join("replacement.tmp");
        std::fs::write(&staged, b"replacement bytes").unwrap();
        staged
    }

    #[test]
    fn unable_to_move_replacement_preserves_staging_and_reports_absent_destination() {
        let staged = staged_file("partial_1176");

        let error =
            ReplaceFailure::from_windows_error(io::Error::from_raw_os_error(1176)).finish(&staged);

        assert_eq!(std::fs::read(&staged).unwrap(), b"replacement bytes");
        assert!(error.to_string().contains("staged replacement preserved"));
        assert!(error.to_string().contains(&staged.display().to_string()));
        assert!(error.to_string().contains("destination may be absent"));
        assert!(!error.to_string().contains("inherited metadata"));
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn unable_to_move_replacement_2_reports_moved_destination_and_inherited_metadata() {
        let staged = staged_file("partial_1177");

        let error =
            ReplaceFailure::from_windows_error(io::Error::from_raw_os_error(1177)).finish(&staged);

        assert_eq!(std::fs::read(&staged).unwrap(), b"replacement bytes");
        assert!(error.to_string().contains("staged replacement preserved"));
        assert!(error.to_string().contains(&staged.display().to_string()));
        assert!(error
            .to_string()
            .contains("original destination may have moved"));
        assert!(error.to_string().contains("inherited metadata"));
        assert!(!error.to_string().contains("destination may be absent"));
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn clean_windows_replace_failure_removes_staged_replacement() {
        let staged = staged_file("clean");

        let error =
            ReplaceFailure::from_windows_error(io::Error::from_raw_os_error(1175)).finish(&staged);

        assert!(!staged.exists());
        assert_eq!(error.raw_os_error(), Some(1175));
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn known_windows_partial_replace_is_typed_as_possibly_published() {
        let staged = staged_file("typed_partial_1176");
        let expected_staged = staged.clone();

        let failure = ReplaceFailure::from_windows_error(io::Error::from_raw_os_error(1176))
            .into_failure(&staged);

        match failure {
            ArtifactWriteFailure::PossiblyPublished { error, staged_path } => {
                assert!(error.to_string().contains("destination may be absent"));
                assert_eq!(staged_path, expected_staged);
            }
            ArtifactWriteFailure::NotPublished(_) => {
                panic!("known partial replacement must be possibly published")
            }
        }
        assert_eq!(std::fs::read(&staged).unwrap(), b"replacement bytes");
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }
}

#[cfg(unix)]
#[cfg(test)]
mod detailed_write_tests {
    use super::{ArtifactDurability, TextArtifact};
    use std::io;

    fn output(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "pyo3_bindgen_artifact_detailed_{name}_{}_{}",
            std::process::id(),
            super::TEMP_FILE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let output = directory.join("bindings.rs");
        (directory, output)
    }

    #[test]
    fn injected_parent_sync_failure_is_reported_after_detailed_publication() {
        let (directory, output) = output("detailed_failure");
        let artifact = TextArtifact::new("detailed replacement");
        super::inject_parent_directory_sync_failure();

        let receipt = artifact.write_detailed(&output).unwrap();

        match receipt.durability {
            ArtifactDurability::DirectorySyncFailed(error) => {
                assert_eq!(error.kind(), io::ErrorKind::Other);
                assert!(error.to_string().contains("injected"));
            }
            durability => panic!("expected durability failure, got {durability:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            artifact.contents()
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn injected_parent_sync_failure_makes_wrapper_return_an_error_after_publication() {
        let (directory, output) = output("wrapper_failure");
        let artifact = TextArtifact::new("wrapper replacement");
        super::inject_parent_directory_sync_failure();

        let error = artifact.write(&output).unwrap_err();

        assert!(error.to_string().contains("replacement already occurred"));
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            artifact.contents()
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}
