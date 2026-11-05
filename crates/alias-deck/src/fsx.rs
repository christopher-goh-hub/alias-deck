use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::Error;

/// `Ok(None)` when the file does not exist, so a first run needs no special case.
pub fn read_optional(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(path)(e)),
    }
}

/// Writes through a temp file in the same directory and renames it over `path`, so a crash
/// leaves either the old file or the new one, never half of one. A symlink is followed, so a
/// dotfiles manager's link to `~/.zshrc` stays a link; the file keeps its permissions.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let target: PathBuf = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path).map_err(Error::io(path))?,
        _ => path.to_owned(),
    };
    let dir = target.parent().ok_or_else(|| Error::io(&target)(io::ErrorKind::InvalidInput.into()))?;
    fs::create_dir_all(dir).map_err(Error::io(dir))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(Error::io(dir))?;
    tmp.write_all(bytes).map_err(Error::io(tmp.path()))?;
    tmp.as_file().sync_all().map_err(Error::io(tmp.path()))?;
    if let Some(perms) = fs::metadata(&target).ok().map(|m| m.permissions()).or_else(new_file_permissions) {
        fs::set_permissions(tmp.path(), perms).map_err(Error::io(tmp.path()))?;
    }
    tmp.persist(&target).map_err(|e| Error::io(&target)(e.error))?;
    Ok(())
}

/// A temp file starts private (0600), but a new init or rc file should read like any other.
#[cfg(unix)]
fn new_file_permissions() -> Option<fs::Permissions> {
    use std::os::unix::fs::PermissionsExt;
    Some(fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn new_file_permissions() -> Option<fs::Permissions> {
    None
}
