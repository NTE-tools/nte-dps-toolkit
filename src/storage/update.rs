use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use semver::Version;
use sha2::{Digest as _, Sha256};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use zip::ZipArchive;

use crate::core::update::{
    AvailableComponentUpdate, UPDATE_HEALTH_MARKER_ENV, UPDATE_SCHEMA_VERSION, UpdateComponent,
    UpdateTransaction, safe_managed_release_path,
};
use crate::platform::update_http;
use crate::storage::io_util::atomic_write_text;
use crate::storage::paths;

const MAX_ARCHIVE_ENTRIES: usize = 4_096;
const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
const UPDATE_ROOT_DIRECTORY: &str = ".update";
const COMPLETED_UPDATE_CLEANUP_DELAY: Duration = Duration::from_secs(1);
const COMPLETED_UPDATE_CLEANUP_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const COMPLETED_UPDATE_CLEANUP_ATTEMPTS: usize = 40;

#[derive(Clone, Debug)]
pub enum PreparedUpdate {
    App {
        version: Version,
        transaction_path: PathBuf,
        updater_path: PathBuf,
    },
}

impl PreparedUpdate {
    pub const fn component(&self) -> UpdateComponent {
        match self {
            Self::App { .. } => UpdateComponent::App,
        }
    }

    pub fn version(&self) -> &Version {
        match self {
            Self::App { version, .. } => version,
        }
    }
}

#[derive(Debug)]
pub enum PrepareUpdateError {
    Download(update_http::HttpError),
    HashMismatch,
    Archive(String),
    UnsafeArchivePath(String),
    UnsupportedArchivePath(String),
    TooManyArchiveEntries,
    ArchiveTooLarge,
    MissingApplication,
    MissingUpdater,
    File(io::Error),
    Transaction(String),
}

impl fmt::Display for PrepareUpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Download(error) => write!(formatter, "{error}"),
            Self::HashMismatch => formatter.write_str("downloaded update hash does not match"),
            Self::Archive(error) => write!(formatter, "update archive is invalid: {error}"),
            Self::UnsafeArchivePath(path) => {
                write!(formatter, "update archive contains an unsafe path: {path}")
            }
            Self::UnsupportedArchivePath(path) => {
                write!(
                    formatter,
                    "update archive contains an unsupported path: {path}"
                )
            }
            Self::TooManyArchiveEntries => {
                formatter.write_str("update archive contains too many entries")
            }
            Self::ArchiveTooLarge => {
                formatter.write_str("expanded update archive exceeds the size limit")
            }
            Self::MissingApplication => {
                formatter.write_str("update archive is missing nte-dps-tool.exe")
            }
            Self::MissingUpdater => {
                formatter.write_str("update archive is missing nte-updater.exe")
            }
            Self::File(error) => write!(formatter, "update file operation failed: {error}"),
            Self::Transaction(error) => write!(formatter, "update transaction is invalid: {error}"),
        }
    }
}

impl std::error::Error for PrepareUpdateError {}

pub fn prepare_update(
    update: &AvailableComponentUpdate,
    progress: impl FnMut(u64, u64),
) -> Result<PreparedUpdate, PrepareUpdateError> {
    match update.component {
        UpdateComponent::App => prepare_app_update(update, progress),
    }
}

fn prepare_app_update(
    update: &AvailableComponentUpdate,
    progress: impl FnMut(u64, u64),
) -> Result<PreparedUpdate, PrepareUpdateError> {
    let install_dir = paths::software_dir()
        .canonicalize()
        .map_err(PrepareUpdateError::File)?;
    let update_root = install_dir.join(UPDATE_ROOT_DIRECTORY);
    let hash_hex = hex::encode(update.artifact_sha256);
    let transaction_id = format!("app-{}-{}", update.version, &hash_hex[..12]);
    let package_path = update_root
        .join("downloads")
        .join(format!("{transaction_id}.zip.part"));
    update_http::download_file(
        &update.artifact_url,
        &package_path,
        update.artifact_size,
        progress,
    )
    .map_err(PrepareUpdateError::Download)?;
    if sha256_file(&package_path)? != update.artifact_sha256 {
        let _ = fs::remove_file(&package_path);
        return Err(PrepareUpdateError::HashMismatch);
    }

    let staging_dir = update_root.join("staging").join(&transaction_id);
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir).map_err(PrepareUpdateError::File)?;
    }
    fs::create_dir_all(&staging_dir).map_err(PrepareUpdateError::File)?;
    let files = extract_release_archive(&package_path, &staging_dir)?;
    if !files
        .iter()
        .any(|path| path == Path::new("nte-dps-tool.exe"))
    {
        return Err(PrepareUpdateError::MissingApplication);
    }
    if !files
        .iter()
        .any(|path| path == Path::new("nte-updater.exe"))
    {
        return Err(PrepareUpdateError::MissingUpdater);
    }

    let health_marker = update_root
        .join("health")
        .join(format!("{transaction_id}.ok"));
    if health_marker.exists() {
        fs::remove_file(&health_marker).map_err(PrepareUpdateError::File)?;
    }
    let transaction = UpdateTransaction {
        schema: UPDATE_SCHEMA_VERSION,
        id: transaction_id.clone(),
        parent_pid: std::process::id(),
        install_dir,
        staging_dir: staging_dir.clone(),
        health_marker,
        files,
    };
    let transaction_text = serde_json::to_string_pretty(&transaction)
        .map_err(|error| PrepareUpdateError::Transaction(error.to_string()))?;
    let transaction_path = update_root
        .join("transactions")
        .join(format!("{transaction_id}.json"));
    atomic_write_text(&transaction_path, &transaction_text)
        .map_err(PrepareUpdateError::Transaction)?;
    let _ = fs::remove_file(package_path);
    Ok(PreparedUpdate::App {
        version: update.version.clone(),
        transaction_path,
        updater_path: staging_dir.join("nte-updater.exe"),
    })
}

pub fn launch_prepared_app_update(update: &PreparedUpdate) -> io::Result<Child> {
    let PreparedUpdate::App {
        transaction_path,
        updater_path,
        ..
    } = update;
    let mut command = Command::new(updater_path);
    command
        .arg("--apply")
        .arg(transaction_path)
        .creation_flags(CREATE_NO_WINDOW);
    command.spawn()
}

pub fn mark_update_healthy_from_environment() -> io::Result<Option<PathBuf>> {
    let Some(marker) = std::env::var_os(UPDATE_HEALTH_MARKER_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    let allowed_root = paths::software_dir()
        .canonicalize()?
        .join(UPDATE_ROOT_DIRECTORY)
        .join("health");
    if marker.parent() != Some(allowed_root.as_path())
        || health_marker_transaction_id(&marker).is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "update health marker is outside the managed directory",
        ));
    }
    fs::create_dir_all(&allowed_root)?;
    let mut file = File::create(&marker)?;
    file.write_all(b"healthy\n")?;
    file.sync_all()?;
    Ok(Some(marker))
}

pub fn cleanup_completed_app_update(marker: PathBuf) {
    let Some(transaction_id) = health_marker_transaction_id(&marker).map(str::to_owned) else {
        return;
    };
    let Some(update_root) = marker.parent().and_then(Path::parent).map(Path::to_owned) else {
        return;
    };
    std::thread::sleep(COMPLETED_UPDATE_CLEANUP_DELAY);
    for _ in 0..COMPLETED_UPDATE_CLEANUP_ATTEMPTS {
        cleanup_completed_transaction(&update_root, &transaction_id, &marker);
        if !update_root.exists() {
            return;
        }
        std::thread::sleep(COMPLETED_UPDATE_CLEANUP_RETRY_INTERVAL);
    }
}

fn cleanup_completed_transaction(update_root: &Path, transaction_id: &str, marker: &Path) {
    let transactions_root = update_root.join("transactions");
    if transactions_root
        .join(format!("{transaction_id}.json"))
        .is_file()
    {
        return;
    }
    let _ = fs::remove_dir_all(update_root.join("staging").join(transaction_id));
    let _ = fs::remove_dir_all(update_root.join("backup").join(transaction_id));
    let _ = fs::remove_file(transactions_root.join(format!("{transaction_id}.log")));
    let _ = fs::remove_file(marker);
    for directory in ["downloads", "staging", "transactions", "health", "backup"] {
        let _ = fs::remove_dir(update_root.join(directory));
    }
    let _ = fs::remove_dir(update_root);
}

fn health_marker_transaction_id(marker: &Path) -> Option<&str> {
    let transaction_id = marker.file_name()?.to_str()?.strip_suffix(".ok")?;
    (!transaction_id.is_empty()
        && transaction_id.len() <= 96
        && transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    .then_some(transaction_id)
}

fn sha256_file(path: &Path) -> Result<[u8; 32], PrepareUpdateError> {
    sha256_file_io(path).map_err(PrepareUpdateError::File)
}

fn sha256_file_io(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}

fn extract_release_archive(
    package_path: &Path,
    staging_dir: &Path,
) -> Result<Vec<PathBuf>, PrepareUpdateError> {
    let package = File::open(package_path).map_err(PrepareUpdateError::File)?;
    let mut archive =
        ZipArchive::new(package).map_err(|error| PrepareUpdateError::Archive(error.to_string()))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(PrepareUpdateError::TooManyArchiveEntries);
    }
    let mut extracted_bytes = 0_u64;
    let mut files = Vec::new();
    let mut paths = HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| PrepareUpdateError::Archive(error.to_string()))?;
        let name = entry
            .name()
            .map_err(|error| PrepareUpdateError::Archive(error.to_string()))?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| PrepareUpdateError::UnsafeArchivePath(name.into_owned()))?
            .to_path_buf();
        if !safe_managed_release_path(&relative) {
            return Err(PrepareUpdateError::UnsupportedArchivePath(
                relative.display().to_string(),
            ));
        }
        if !paths.insert(release_path_key(&relative)) {
            return Err(PrepareUpdateError::Archive(format!(
                "duplicate path {}",
                relative.display()
            )));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(PrepareUpdateError::UnsafeArchivePath(
                relative.display().to_string(),
            ));
        }
        extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .ok_or(PrepareUpdateError::ArchiveTooLarge)?;
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err(PrepareUpdateError::ArchiveTooLarge);
        }
        let target = staging_dir.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(PrepareUpdateError::File)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(PrepareUpdateError::File)?;
        }
        let mut output = File::create(&target).map_err(PrepareUpdateError::File)?;
        io::copy(&mut entry, &mut output).map_err(PrepareUpdateError::File)?;
        output.flush().map_err(PrepareUpdateError::File)?;
        output.sync_all().map_err(PrepareUpdateError::File)?;
        files.push(relative);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn release_path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update_test_directory(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("nte-update-{name}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        path
    }

    #[test]
    fn release_path_policy_accepts_signed_package_files_without_filename_allowlist() {
        assert!(safe_managed_release_path(Path::new("nte-dps-tool.exe")));
        assert!(safe_managed_release_path(Path::new("nte-mod-loader.exe")));
        assert!(safe_managed_release_path(Path::new("future-sidecar.exe")));
        assert!(safe_managed_release_path(Path::new("plugins/dwmapi.dll")));
        assert!(safe_managed_release_path(Path::new("licenses/dep/LICENSE")));
    }

    #[test]
    fn release_path_policy_rejects_protected_user_and_update_state() {
        assert!(!safe_managed_release_path(Path::new("config.json")));
        assert!(!safe_managed_release_path(Path::new(
            "history/session.json"
        )));
        assert!(!safe_managed_release_path(Path::new("logs/capture.pcapng")));
        assert!(!safe_managed_release_path(Path::new(
            ".update/staging/file"
        )));
        assert!(!safe_managed_release_path(Path::new(
            "plugins/mods-plugin.state.json"
        )));
        assert!(!safe_managed_release_path(Path::new(
            "plugins/equipment-plugin.state.json"
        )));
    }

    #[test]
    fn relative_path_validation_rejects_parent_traversal() {
        assert!(!safe_managed_release_path(Path::new("../nte-dps-tool.exe")));
        assert!(safe_managed_release_path(Path::new("plugins/dwmapi.dll")));
    }

    #[test]
    fn archive_path_keys_follow_windows_case_insensitive_semantics() {
        assert_eq!(
            release_path_key(Path::new("plugins/dwmapi.dll")),
            release_path_key(Path::new("PLUGINS/DWMAPI.DLL"))
        );
    }

    #[test]
    fn verified_app_archive_extracts_new_root_sidecar_without_client_filename_change() {
        let root = update_test_directory("dynamic-release-path");
        let package = root.join("release.zip");
        let file = File::create(&package).expect("release archive should be created");
        let mut archive = zip::ZipWriter::new(file);
        for (path, contents) in [
            ("nte-dps-tool.exe", b"app".as_slice()),
            ("nte-mod-loader.exe", b"loader".as_slice()),
            ("nte-updater.exe", b"updater".as_slice()),
            ("licenses/说明.txt", b"notice".as_slice()),
        ] {
            archive
                .start_file(path, zip::write::SimpleFileOptions::default())
                .expect("release entry should start");
            archive
                .write_all(contents)
                .expect("release entry should be written");
        }
        archive.finish().expect("release archive should finish");

        let staging = root.join("staging");
        fs::create_dir_all(&staging).expect("staging directory should be created");
        let files = extract_release_archive(&package, &staging)
            .expect("verified release archive should extract");

        assert!(files.contains(&PathBuf::from("nte-dps-tool.exe")));
        assert!(files.contains(&PathBuf::from("nte-mod-loader.exe")));
        assert!(files.contains(&PathBuf::from("nte-updater.exe")));
        assert_eq!(
            fs::read(staging.join("nte-mod-loader.exe")).unwrap(),
            b"loader"
        );
        assert_eq!(
            fs::read(staging.join("licenses/说明.txt")).unwrap(),
            b"notice"
        );
        fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn release_archive_rejects_unsafe_paths_and_symlinks_without_writing_files() {
        let root = update_test_directory("unsafe-release-entries");
        let package = root.join("release.zip");
        let staging = root.join("staging");
        fs::create_dir_all(&staging).unwrap();

        for (name, symlink) in [("../outside.exe", false), ("linked.exe", true)] {
            let mut archive = zip::ZipWriter::new(File::create(&package).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            if symlink {
                archive
                    .add_symlink(name, "../outside.exe", options)
                    .unwrap();
            } else {
                archive.start_file(name, options).unwrap();
                archive.write_all(b"untrusted").unwrap();
            }
            archive.finish().unwrap();

            let error = extract_release_archive(&package, &staging).unwrap_err();
            assert!(matches!(error, PrepareUpdateError::UnsafeArchivePath(_)));
            assert_eq!(fs::read_dir(&staging).unwrap().count(), 0);
            assert!(!root.join("outside.exe").exists());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_update_cleanup_removes_the_managed_root() {
        let root = update_test_directory("completed-cleanup");
        let update_root = root.join(UPDATE_ROOT_DIRECTORY);
        fs::create_dir_all(update_root.join("staging/app-0.3.6-hash")).unwrap();
        fs::create_dir_all(update_root.join("transactions")).unwrap();
        fs::create_dir_all(update_root.join("health")).unwrap();
        fs::write(
            update_root.join("transactions/app-0.3.6-hash.log"),
            b"update completed\n",
        )
        .unwrap();
        fs::write(update_root.join("health/app-0.3.6-hash.ok"), b"healthy\n").unwrap();

        let marker = update_root.join("health/app-0.3.6-hash.ok");
        cleanup_completed_transaction(&update_root, "app-0.3.6-hash", &marker);

        assert!(!update_root.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transaction_cleanup_does_not_touch_another_active_update() {
        let root = update_test_directory("transaction-cleanup");
        let update_root = root.join(UPDATE_ROOT_DIRECTORY);
        let completed_id = "app-0.3.6-hash";
        let marker = update_root
            .join("health")
            .join(format!("{completed_id}.ok"));
        fs::create_dir_all(update_root.join("staging").join(completed_id)).unwrap();
        fs::create_dir_all(update_root.join("backup").join(completed_id)).unwrap();
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::create_dir_all(update_root.join("transactions")).unwrap();
        fs::write(
            update_root
                .join("transactions")
                .join(format!("{completed_id}.log")),
            b"update completed\n",
        )
        .unwrap();
        fs::write(&marker, b"healthy\n").unwrap();

        let active_staging = update_root.join("staging/app-0.3.7-hash");
        let active_download = update_root.join("downloads/app-0.3.7-hash.zip.part");
        fs::create_dir_all(&active_staging).unwrap();
        fs::create_dir_all(active_download.parent().unwrap()).unwrap();
        fs::write(&active_download, b"partial package").unwrap();

        cleanup_completed_transaction(&update_root, completed_id, &marker);

        assert!(!update_root.join("staging").join(completed_id).exists());
        assert!(!update_root.join("backup").join(completed_id).exists());
        assert!(!marker.exists());
        assert!(active_staging.is_dir());
        assert!(active_download.is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn health_marker_transaction_id_rejects_unsafe_names() {
        assert_eq!(
            health_marker_transaction_id(Path::new("health/app-0.3.6-0123456789ab.ok")),
            Some("app-0.3.6-0123456789ab")
        );
        assert_eq!(
            health_marker_transaction_id(Path::new("health/app update.ok")),
            None
        );
        assert_eq!(
            health_marker_transaction_id(Path::new("health/app-0.3.6.json")),
            None
        );
    }
}
