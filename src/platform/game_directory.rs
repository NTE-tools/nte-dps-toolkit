//! Game installation directory lookup for optional UE Tools proxy deployment.
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, REG_SZ, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RegGetValueW,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameRegion {
    China,
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameDirectoryError {
    GameInstallationNotFound,
    InvalidGameDirectory,
    Registry(String),
    FileSystem(String),
}

impl fmt::Display for GameDirectoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GameInstallationNotFound => {
                formatter.write_str("game installation was not found")
            }
            Self::InvalidGameDirectory => {
                formatter.write_str("selected directory does not contain HTGame.exe")
            }
            Self::Registry(error) => write!(formatter, "game registry lookup failed: {error}"),
            Self::FileSystem(error) => write!(formatter, "game directory lookup failed: {error}"),
        }
    }
}

impl std::error::Error for GameDirectoryError {}

const GAME_INSTALL_REGISTRY_KEYS: [(GameRegion, &str); 2] = [
    (
        GameRegion::China,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\YH",
    ),
    (
        GameRegion::Global,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\NTEGlobal",
    ),
];
const GAME_BINARY_RELATIVE_PATH: &str = r"Client\WindowsNoEditor\HT\Binaries\Win64";
const GAME_EXECUTABLE_NAME: &str = "HTGame.exe";

/// Revalidate an explicit target on every deployment; never fall back after an invalid selection.
pub fn toolkit_game_directory(manual: Option<&Path>) -> Result<PathBuf, GameDirectoryError> {
    match manual {
        Some(path) => resolve_manual_game_directory(path),
        None => automatic_toolkit_game_directory(),
    }
}

/// Automatic proxy deployment never guesses between installations or uses a stale manual path.
pub fn automatic_toolkit_game_directory() -> Result<PathBuf, GameDirectoryError> {
    let directories = game_installation_directories()?;
    if directories.len() != 1 {
        return Err(GameDirectoryError::GameInstallationNotFound);
    }
    Ok(directories[0].1.clone())
}

pub fn resolve_manual_game_directory(selected: &Path) -> Result<PathBuf, GameDirectoryError> {
    for directory in [
        selected.to_path_buf(),
        selected.join(GAME_BINARY_RELATIVE_PATH),
        selected
            .join("Neverness To Everness")
            .join(GAME_BINARY_RELATIVE_PATH),
    ] {
        if directory.join(GAME_EXECUTABLE_NAME).is_file() {
            return directory.canonicalize().map_err(file_system_error);
        }
    }
    Err(GameDirectoryError::InvalidGameDirectory)
}

fn game_installation_directories() -> Result<Vec<(GameRegion, PathBuf)>, GameDirectoryError> {
    let mut directories = Vec::new();
    for (region, key) in GAME_INSTALL_REGISTRY_KEYS {
        let Some(root) = read_registry_string(key, "InstallLocation")? else {
            continue;
        };
        let root = PathBuf::from(root.trim().trim_matches('"'));
        for directory in [
            root.join(GAME_BINARY_RELATIVE_PATH),
            root.join("Neverness To Everness")
                .join(GAME_BINARY_RELATIVE_PATH),
        ] {
            if directory.join(GAME_EXECUTABLE_NAME).is_file()
                && !directories.iter().any(|(_, known)| known == &directory)
            {
                directories.push((region, directory));
                break;
            }
        }
    }
    if directories.is_empty() {
        return Err(GameDirectoryError::GameInstallationNotFound);
    }
    Ok(directories)
}

fn read_registry_string(subkey: &str, value: &str) -> Result<Option<String>, GameDirectoryError> {
    let subkey = wide_null(subkey);
    let value = wide_null(value);
    let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6432KEY;
    let mut value_type = 0;
    let mut byte_len = 0;
    // SAFETY: both strings are NUL-terminated, output pointers are valid, and
    // the first call requests only the required byte count.
    let first = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            flags,
            &mut value_type,
            ptr::null_mut(),
            &mut byte_len,
        )
    };
    if first == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if first != 0 {
        return Err(GameDirectoryError::Registry(format!(
            "registry query failed with error code {first}"
        )));
    }
    if value_type != REG_SZ || byte_len < 2 || byte_len % 2 != 0 {
        return Err(GameDirectoryError::Registry(
            "game install registry value has an invalid type or length".to_owned(),
        ));
    }
    let mut buffer = vec![0_u16; byte_len as usize / 2];
    // SAFETY: the buffer has the exact byte capacity reported by the first
    // query and the same NUL-terminated key and value names remain alive.
    let second = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            flags,
            &mut value_type,
            buffer.as_mut_ptr().cast(),
            &mut byte_len,
        )
    };
    if second != 0 {
        return Err(GameDirectoryError::Registry(format!(
            "registry value read failed with error code {second}"
        )));
    }
    if value_type != REG_SZ
        || byte_len < 2
        || byte_len % 2 != 0
        || byte_len as usize > buffer.len() * 2
    {
        return Err(GameDirectoryError::Registry(
            "game install registry value changed to an invalid type or length".to_owned(),
        ));
    }
    buffer.truncate(byte_len as usize / 2);
    if buffer.last() != Some(&0) {
        return Err(GameDirectoryError::Registry(
            "game install registry value is not a terminated string".to_owned(),
        ));
    }
    buffer.pop();
    String::from_utf16(&buffer).map(Some).map_err(|_| {
        GameDirectoryError::Registry("game install registry value is not valid UTF-16".to_owned())
    })
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn file_system_error(error: io::Error) -> GameDirectoryError {
    GameDirectoryError::FileSystem(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_directory(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nte-game-directory-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test directory should be created");
        path
    }

    #[test]
    fn manual_game_directory_accepts_install_root_or_binary_directory() {
        let root = test_directory("manual-game-directory");
        let binary = root.join(GAME_BINARY_RELATIVE_PATH);
        fs::create_dir_all(&binary).unwrap();
        fs::write(binary.join(GAME_EXECUTABLE_NAME), b"fixture").unwrap();
        let canonical_binary = binary.canonicalize().unwrap();

        assert_eq!(
            resolve_manual_game_directory(&root).unwrap(),
            canonical_binary
        );
        assert_eq!(
            resolve_manual_game_directory(&binary).unwrap(),
            canonical_binary
        );
        assert_eq!(
            resolve_manual_game_directory(&root.join("missing")),
            Err(GameDirectoryError::InvalidGameDirectory)
        );

        // Explicit selection bypasses automatic discovery, but is revalidated every time.
        assert_eq!(
            toolkit_game_directory(Some(&root)).unwrap(),
            canonical_binary
        );
        fs::remove_file(binary.join(GAME_EXECUTABLE_NAME)).unwrap();
        assert_eq!(
            toolkit_game_directory(Some(&root)),
            Err(GameDirectoryError::InvalidGameDirectory)
        );
        fs::create_dir(binary.join(GAME_EXECUTABLE_NAME)).unwrap();
        assert_eq!(
            toolkit_game_directory(Some(&binary)),
            Err(GameDirectoryError::InvalidGameDirectory)
        );
        fs::remove_dir_all(root).unwrap();
    }
}
