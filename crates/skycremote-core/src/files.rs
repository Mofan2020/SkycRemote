//! The files of the machine that is being controlled.
//!
//! The other end may ask what is in a folder, ask for a file to be sent
//! to it, and send a file over to be written.  All of it happens inside
//! the home folder of the person who is sharing: a path that points
//! anywhere else is refused, because the person on the other end
//! should not be able to walk into the system folders of this machine
//! just because they know the access code.
//!
//! The bytes go over the control channel in pieces.  The channel is
//! reliable and ordered, so a file arrives whole or not at all.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use crate::protocol::FileEntry;

/// How many bytes go in one piece.
///
/// The control channel carries the pieces as text, so they are made
/// bigger than a screen chunk but not so big that one piece fills the
/// channel for a noticeable while.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// What can go wrong while serving the files.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// The path is outside the folder that is shared.
    #[error("that path is not inside the folder that is shared")]
    Outside,
    /// The path is not a folder, so it cannot be listed.
    #[error("that is not a folder")]
    NotAFolder,
    /// There is nothing at that path.
    #[error("there is nothing at that path")]
    NotFound,
    /// The machine would not do it.
    #[error("the file could not be used: {0}")]
    Io(String),
}

/// The folder the files come from.
///
/// It is the home folder of the person who is sharing, because that is
/// where the files people send each other live.
pub fn root() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Turn a path that came over the wire into a real path inside the
/// shared folder.
///
/// The path may be absolute (inside the shared folder) or relative to
/// it.  Anything that climbs out with .. is refused, and so is
/// anything that is not a plain path.
pub fn resolve(path: &str) -> Result<PathBuf, FileError> {
    let root = root();
    let trimmed = path.trim();
    let candidate = if trimmed.is_empty() || trimmed == "/" || trimmed == "~" {
        root.clone()
    } else {
        let raw = Path::new(trimmed);
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            root.join(raw)
        }
    };

    // The path is cleaned by hand rather than with canonicalize, because
    // the file may not exist yet: an upload names a file that is going
    // to be written.
    let out = normalize(&candidate);
    // Anything that is still outside after the cleaning is not wanted.
    if !out.starts_with(&root) {
        return Err(FileError::Outside);
    }
    Ok(out)
}

/// Clean a path: no "." parts, and every ".." takes one part away.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

/// The path of a file, as the other end should know it.
///
/// It is relative to the shared folder, so that the other end does not
/// learn where the home folder is, and so that an empty path means the
/// shared folder itself.
pub fn relative(path: &Path) -> String {
    let root = root();
    match path.strip_prefix(&root) {
        Ok(rest) => rest.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

/// List what is in a folder.
///
/// The name of the folder itself comes back too, so that the other end
/// can show where it is.
pub fn list(path: &str) -> Result<(PathBuf, Vec<FileEntry>), FileError> {
    let real = resolve(path)?;
    if !real.exists() {
        return Err(FileError::NotFound);
    }
    if !real.is_dir() {
        return Err(FileError::NotAFolder);
    }
    let mut entries = Vec::new();
    let reader = std::fs::read_dir(&real).map_err(|error| FileError::Io(error.to_string()))?;
    for item in reader {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().to_string();
        let is_dir = item.path().is_dir();
        let size = if is_dir {
            0
        } else {
            item.metadata().map(|meta| meta.len()).unwrap_or(0)
        };
        entries.push(FileEntry { name, is_dir, size });
    }
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok((real, entries))
}

/// Read one piece of a file.
///
/// The piece starts at the offset and is as long as it can be.
pub fn read_chunk(path: &str, offset: u64, length: usize) -> Result<Vec<u8>, FileError> {
    let real = resolve(path)?;
    let mut file = File::open(&real).map_err(|error| FileError::Io(error.to_string()))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| FileError::Io(error.to_string()))?;
    let mut buffer = vec![0u8; length];
    let mut filled = 0;
    while filled < length {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) => return Err(FileError::Io(error.to_string())),
        }
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// How many bytes a file holds.
pub fn size_of(path: &str) -> Result<u64, FileError> {
    let real = resolve(path)?;
    let meta = std::fs::metadata(&real).map_err(|error| FileError::Io(error.to_string()))?;
    Ok(meta.len())
}

/// Make the file that an upload is going into.
///
/// The folder it goes into has to be there already: the other end may
/// not make folders on this machine.
pub fn prepare_upload(path: &str) -> Result<(), FileError> {
    let real = resolve(path)?;
    if let Some(parent) = real.parent() {
        if !parent.exists() {
            return Err(FileError::NotFound);
        }
    }
    let mut file = File::create(&real).map_err(|error| FileError::Io(error.to_string()))?;
    file.flush()
        .map_err(|error| FileError::Io(error.to_string()))
}

/// Write one piece of an uploaded file at its place.
pub fn write_chunk(path: &str, offset: u64, data: &[u8]) -> Result<(), FileError> {
    let real = resolve(path)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&real)
        .map_err(|error| FileError::Io(error.to_string()))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| FileError::Io(error.to_string()))?;
    file.write_all(data)
        .map_err(|error| FileError::Io(error.to_string()))
}

/// Where a file that was sent over is put on this machine.
///
/// It is a folder of its own under the home folder, so that the files
/// that came in are easy to find and do not mix with the ones that were
/// already there.
pub fn incoming_folder() -> PathBuf {
    let folder = root().join("Downloads").join("SkycRemote");
    let _ = std::fs::create_dir_all(&folder);
    folder
}

/// The place a file that was sent over is written to.
///
/// The name of the file is made safe: nothing of the other end's
/// choosing can make it point somewhere else.
pub fn incoming_path(name: &str) -> PathBuf {
    let safe: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file")
        .chars()
        .map(|character| {
            if character.is_alphanumeric()
                || character == '.'
                || character == '-'
                || character == '_'
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.trim().is_empty() {
        "file".to_string()
    } else {
        safe
    };
    incoming_folder().join(safe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_inside_the_home_folder_is_accepted() {
        let real = resolve("Documents").expect("Documents is inside the home folder");
        assert!(real.starts_with(root()));
    }

    #[test]
    fn climbing_out_of_the_home_folder_is_refused() {
        let result = resolve("../../../../etc/passwd");
        assert!(matches!(result, Err(FileError::Outside)), "{result:?}");
    }

    #[test]
    fn an_absolute_path_inside_the_home_folder_is_kept() {
        let inside = root().join("Documents").join("note.txt");
        let real = resolve(inside.to_str().unwrap_or("")).expect("inside the home folder");
        assert_eq!(real, inside);
    }

    #[test]
    fn the_name_of_a_sent_file_cannot_point_outside() {
        let path = incoming_path("../../evil.txt");
        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .to_string();
        assert_eq!(name, "evil.txt");
        assert!(path.starts_with(incoming_folder()));
    }

    #[test]
    fn a_listing_is_readable() {
        let folder = incoming_folder();
        let (real, entries) = list(folder.to_str().unwrap_or("")).expect("the folder is there");
        assert!(real.is_dir());
        assert!(entries.iter().all(|entry| !entry.name.is_empty()));
    }
}
