use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

use crate::core::update::{
    UPDATE_HEALTH_MARKER_ENV, UPDATE_SCHEMA_VERSION, UpdateTransaction, safe_managed_release_path,
};

const MAX_TRANSACTION_BYTES: u64 = 1024 * 1024;
const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(120);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub enum InstallUpdateError {
    InvalidArguments,
    InvalidTransaction(String),
    ParentExitTimeout,
    File(io::Error),
    NewApplicationExited(Option<i32>),
    HealthTimeout,
    Rollback(String),
}

impl fmt::Display for InstallUpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArguments => formatter.write_str("expected --apply TRANSACTION_PATH"),
            Self::InvalidTransaction(error) => write!(formatter, "invalid transaction: {error}"),
            Self::ParentExitTimeout => {
                formatter.write_str("main application did not exit before the update timeout")
            }
            Self::File(error) => write!(formatter, "update file operation failed: {error}"),
            Self::NewApplicationExited(code) => {
                write!(
                    formatter,
                    "updated application exited before reporting healthy (code {code:?})"
                )
            }
            Self::HealthTimeout => {
                formatter.write_str("updated application did not report healthy before timeout")
            }
            Self::Rollback(error) => write!(formatter, "update rollback failed: {error}"),
        }
    }
}

impl std::error::Error for InstallUpdateError {}

pub fn run_from_args() -> Result<(), InstallUpdateError> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--apply")) {
        return Err(InstallUpdateError::InvalidArguments);
    }
    let transaction_path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or(InstallUpdateError::InvalidArguments)?;
    if arguments.next().is_some() {
        return Err(InstallUpdateError::InvalidArguments);
    }
    run_transaction(&transaction_path)
}

pub fn updater_log_path_from_args() -> Option<PathBuf> {
    let mut arguments = std::env::args_os().skip(1);
    (arguments.next().as_deref() == Some(std::ffi::OsStr::new("--apply")))
        .then(|| arguments.next().map(PathBuf::from))
        .flatten()
        .map(|path| path.with_extension("log"))
}

pub fn append_updater_log(path: &Path, message: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

fn run_transaction(transaction_path: &Path) -> Result<(), InstallUpdateError> {
    let transaction = read_transaction(transaction_path)?;
    let updater_exe = std::env::current_exe().map_err(InstallUpdateError::File)?;
    let validated = ValidatedTransaction::new(transaction_path, transaction, &updater_exe)?;
    wait_for_process_exit(validated.transaction.parent_pid)?;
    let applied = match install_files(&validated) {
        Ok(applied) => applied,
        Err(error) => {
            let _ = launch_application(&validated, false);
            return Err(error);
        }
    };
    let mut child = match launch_application(&validated, true) {
        Ok(child) => child,
        Err(error) => {
            rollback_files(&validated, &applied)?;
            launch_application(&validated, false).map_err(InstallUpdateError::File)?;
            return Err(InstallUpdateError::File(error));
        }
    };
    match wait_for_health(&mut child, &validated.transaction.health_marker) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&validated.backup_dir);
            let _ = fs::remove_file(transaction_path);
            Ok(())
        }
        Err(health_error) => {
            let _ = child.kill();
            let _ = child.wait();
            rollback_files(&validated, &applied)?;
            launch_application(&validated, false).map_err(InstallUpdateError::File)?;
            Err(health_error)
        }
    }
}

fn read_transaction(path: &Path) -> Result<UpdateTransaction, InstallUpdateError> {
    let file = File::open(path).map_err(InstallUpdateError::File)?;
    let metadata = file.metadata().map_err(InstallUpdateError::File)?;
    if !metadata.is_file() || metadata.len() > MAX_TRANSACTION_BYTES {
        return Err(InstallUpdateError::InvalidTransaction(
            "transaction file exceeds the size limit".to_owned(),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_TRANSACTION_BYTES) as usize);
    file.take(MAX_TRANSACTION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(InstallUpdateError::File)?;
    if bytes.len() as u64 > MAX_TRANSACTION_BYTES {
        return Err(InstallUpdateError::InvalidTransaction(
            "transaction file exceeds the size limit".to_owned(),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| InstallUpdateError::InvalidTransaction(error.to_string()))
}

struct ValidatedTransaction {
    transaction: UpdateTransaction,
    backup_dir: PathBuf,
}

impl ValidatedTransaction {
    fn new(
        transaction_path: &Path,
        mut transaction: UpdateTransaction,
        updater_exe: &Path,
    ) -> Result<Self, InstallUpdateError> {
        if transaction.schema != UPDATE_SCHEMA_VERSION {
            return Err(InstallUpdateError::InvalidTransaction(format!(
                "unsupported schema {}",
                transaction.schema
            )));
        }
        if !valid_transaction_id(&transaction.id) {
            return Err(InstallUpdateError::InvalidTransaction(
                "invalid transaction ID".to_owned(),
            ));
        }
        let (expected_install_dir, expected_staging_dir, managed_root) =
            managed_roots_from_updater(updater_exe)?;
        let install_dir = transaction
            .install_dir
            .canonicalize()
            .map_err(InstallUpdateError::File)?;
        if install_dir != expected_install_dir {
            return Err(InstallUpdateError::InvalidTransaction(
                "install directory does not match the updater root".to_owned(),
            ));
        }
        let staging_dir = transaction
            .staging_dir
            .canonicalize()
            .map_err(InstallUpdateError::File)?;
        if staging_dir != expected_staging_dir
            || staging_dir.file_name() != Some(std::ffi::OsStr::new(&transaction.id))
        {
            return Err(InstallUpdateError::InvalidTransaction(
                "staging directory does not match the updater staging root".to_owned(),
            ));
        }
        let transaction_file = transaction_path
            .canonicalize()
            .map_err(InstallUpdateError::File)?;
        let expected_transaction_file = managed_root
            .join("transactions")
            .join(format!("{}.json", transaction.id));
        if transaction_file != expected_transaction_file {
            return Err(InstallUpdateError::InvalidTransaction(
                "transaction file is outside the managed update root".to_owned(),
            ));
        }
        let expected_health_dir = managed_root.join("health");
        let expected_health_name = format!("{}.ok", transaction.id);
        let has_parent_dir = transaction
            .health_marker
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
        let health_parent_matches = match transaction.health_marker.parent() {
            Some(parent) if parent == expected_health_dir.as_path() => true,
            Some(parent) => {
                parent.file_name() == Some(std::ffi::OsStr::new("health"))
                    && parent
                        .parent()
                        .and_then(|grandparent| grandparent.canonicalize().ok())
                        .is_some_and(|actual_root| actual_root == managed_root)
            }
            None => false,
        };
        if has_parent_dir
            || !health_parent_matches
            || transaction.health_marker.file_name()
                != Some(std::ffi::OsStr::new(&expected_health_name))
        {
            return Err(InstallUpdateError::InvalidTransaction(
                "health marker is outside the managed update root".to_owned(),
            ));
        }
        transaction.health_marker = expected_health_dir.join(expected_health_name);
        if transaction.files.is_empty() || transaction.files.len() > 4_096 {
            return Err(InstallUpdateError::InvalidTransaction(
                "managed file list is empty or too large".to_owned(),
            ));
        }
        let mut unique = HashSet::new();
        for relative in &transaction.files {
            if !safe_managed_release_path(relative) || !unique.insert(relative.clone()) {
                return Err(InstallUpdateError::InvalidTransaction(format!(
                    "unsafe or duplicate managed path {}",
                    relative.display()
                )));
            }
            if !staging_dir.join(relative).is_file() {
                return Err(InstallUpdateError::InvalidTransaction(format!(
                    "staged file is missing: {}",
                    relative.display()
                )));
            }
        }
        transaction.files.sort_by_key(|path| {
            (
                path == Path::new("nte-dps-tool.exe"),
                path.to_string_lossy().into_owned(),
            )
        });
        transaction.install_dir = install_dir;
        transaction.staging_dir = staging_dir;
        let backup_dir = managed_root.join("backup").join(&transaction.id);
        Ok(Self {
            transaction,
            backup_dir,
        })
    }
}

fn managed_roots_from_updater(
    updater_exe: &Path,
) -> Result<(PathBuf, PathBuf, PathBuf), InstallUpdateError> {
    let updater_path = updater_exe
        .canonicalize()
        .map_err(InstallUpdateError::File)?;
    let staging_dir = updater_path.parent().ok_or_else(|| {
        InstallUpdateError::InvalidTransaction(
            "updater binary is outside the managed staging directory".to_owned(),
        )
    })?;
    let staging_root = staging_dir
        .parent()
        .filter(|path| path.file_name() == Some(std::ffi::OsStr::new("staging")))
        .ok_or_else(|| {
            InstallUpdateError::InvalidTransaction(
                "updater binary is outside the managed staging directory".to_owned(),
            )
        })?;
    let managed_root = staging_root
        .parent()
        .filter(|path| path.file_name() == Some(std::ffi::OsStr::new(".update")))
        .ok_or_else(|| {
            InstallUpdateError::InvalidTransaction(
                "updater binary is outside the managed staging directory".to_owned(),
            )
        })?;
    let install_dir = managed_root.parent().ok_or_else(|| {
        InstallUpdateError::InvalidTransaction(
            "updater binary is outside the managed staging directory".to_owned(),
        )
    })?;
    Ok((
        install_dir.to_path_buf(),
        staging_dir.to_path_buf(),
        managed_root.to_path_buf(),
    ))
}

#[derive(Clone, Debug)]
struct AppliedFile {
    relative: PathBuf,
    existed: bool,
}

fn install_files(validated: &ValidatedTransaction) -> Result<Vec<AppliedFile>, InstallUpdateError> {
    if validated.backup_dir.exists() {
        fs::remove_dir_all(&validated.backup_dir).map_err(InstallUpdateError::File)?;
    }
    fs::create_dir_all(&validated.backup_dir).map_err(InstallUpdateError::File)?;
    let mut applied = Vec::new();
    for relative in &validated.transaction.files {
        let source = validated.transaction.staging_dir.join(relative);
        let target = validated.transaction.install_dir.join(relative);
        let backup = validated.backup_dir.join(relative);
        let existed = target.is_file();
        if existed && let Err(error) = copy_synced(&target, &backup) {
            return match rollback_files(validated, &applied) {
                Ok(()) => Err(InstallUpdateError::File(error)),
                Err(rollback_error) => Err(rollback_error),
            };
        }
        if let Err(error) = replace_from_source(&source, &target, &validated.transaction.id) {
            let rollback = rollback_files(validated, &applied);
            return match rollback {
                Ok(()) => Err(InstallUpdateError::File(error)),
                Err(rollback_error) => Err(rollback_error),
            };
        }
        applied.push(AppliedFile {
            relative: relative.clone(),
            existed,
        });
    }
    Ok(applied)
}

fn rollback_files(
    validated: &ValidatedTransaction,
    applied: &[AppliedFile],
) -> Result<(), InstallUpdateError> {
    let mut failures = Vec::new();
    for file in applied.iter().rev() {
        let target = validated.transaction.install_dir.join(&file.relative);
        if file.existed {
            let backup = validated.backup_dir.join(&file.relative);
            if let Err(error) = replace_from_source(&backup, &target, &validated.transaction.id) {
                failures.push(format!("{}: {error}", file.relative.display()));
            }
        } else if let Err(error) = fs::remove_file(&target)
            && error.kind() != io::ErrorKind::NotFound
        {
            failures.push(format!("{}: {error}", file.relative.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(InstallUpdateError::Rollback(failures.join("; ")))
    }
}

fn valid_transaction_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn replace_from_source(source: &Path, target: &Path, transaction_id: &str) -> io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "managed target has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let file_name = target
        .file_name()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "managed target has no file name",
            )
        })?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.{transaction_id}.tmp"));
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    copy_synced(source, &temporary)?;
    let temporary_wide = wide_null(&temporary);
    let target_wide = wide_null(target);
    // SAFETY: Both path buffers are null-terminated and remain alive for the call.
    let result = unsafe {
        MoveFileExW(
            temporary_wide.as_ptr(),
            target_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(temporary);
        Err(error)
    } else {
        Ok(())
    }
}

fn copy_synced(source: &Path, destination: &Path) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    File::options().write(true).open(destination)?.sync_all()
}

fn wait_for_process_exit(pid: u32) -> Result<(), InstallUpdateError> {
    // SAFETY: The PID comes from the signed transaction created by the parent process. The handle
    // is wrapped immediately and used only for waiting.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            return Ok(());
        }
        return Err(InstallUpdateError::File(error));
    }
    let handle = ProcessHandle(handle);
    // SAFETY: `handle` is a live synchronization handle.
    let result = unsafe { WaitForSingleObject(handle.0, PARENT_EXIT_TIMEOUT.as_millis() as u32) };
    match result {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(InstallUpdateError::ParentExitTimeout),
        _ => Err(InstallUpdateError::File(io::Error::last_os_error())),
    }
}

fn launch_application(validated: &ValidatedTransaction, health_check: bool) -> io::Result<Child> {
    let application = validated.transaction.install_dir.join("nte-dps-tool.exe");
    let mut command = Command::new(application);
    command.current_dir(&validated.transaction.install_dir);
    if health_check {
        if let Some(parent) = validated.transaction.health_marker.parent() {
            fs::create_dir_all(parent)?;
        }
        let _ = fs::remove_file(&validated.transaction.health_marker);
        command.env(
            UPDATE_HEALTH_MARKER_ENV,
            &validated.transaction.health_marker,
        );
    }
    command.spawn()
}

fn wait_for_health(child: &mut Child, marker: &Path) -> Result<(), InstallUpdateError> {
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        if marker.is_file() {
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(InstallUpdateError::File)? {
            return Err(InstallUpdateError::NewApplicationExited(status.code()));
        }
        if Instant::now() >= deadline {
            return Err(InstallUpdateError::HealthTimeout);
        }
        thread::sleep(HEALTH_POLL_INTERVAL);
    }
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: The wrapper owns this non-null process handle and closes it exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn wide_null(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_id_validation_accepts_release_ids_and_rejects_paths() {
        assert!(valid_transaction_id("app-0.3.5-aabbccddeeff"));
        assert!(!valid_transaction_id(""));
        assert!(!valid_transaction_id("../app-0.3.5"));
        assert!(!valid_transaction_id("app/0.3.5"));
    }

    #[test]
    fn atomic_replacement_overwrites_the_managed_file() {
        let root = std::env::temp_dir().join(format!(
            "nte-update-replace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("test directory should be created");
        let source = root.join("staged.exe");
        let target = root.join("installed.exe");
        fs::write(&source, b"new").expect("staged file should be written");
        fs::write(&target, b"old").expect("installed file should be written");

        replace_from_source(&source, &target, "test-transaction")
            .expect("managed file should be replaced");

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(source.is_file());
        fs::remove_dir_all(root).expect("test directory should be removed");
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nte-update-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("test directory should be created");
        dir
    }

    #[test]
    fn read_transaction_rejects_oversized_file() {
        let root = unique_test_dir("oversized-tx");
        let path = root.join("tx.json");
        let file = File::create(&path).expect("test file should be created");
        file.set_len(MAX_TRANSACTION_BYTES + 1)
            .expect("oversized length should be set");

        let error = read_transaction(&path).expect_err("oversized transaction must fail");
        assert!(matches!(error, InstallUpdateError::InvalidTransaction(_)));
        fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn validated_transaction_anchors_install_dir_and_rejects_escapes() {
        let root = unique_test_dir("anchored-tx");
        let install_dir = root.join("app");
        let other_dir = root.join("other");
        let tx_id = "app-0.4.0-aabbccddeeff";
        let staging_dir = install_dir.join(".update").join("staging").join(tx_id);
        let tx_dir = install_dir.join(".update").join("transactions");
        let health_dir = install_dir.join(".update").join("health");
        fs::create_dir_all(&staging_dir).unwrap();
        fs::create_dir_all(&tx_dir).unwrap();
        fs::create_dir_all(&other_dir).unwrap();

        let updater_exe = staging_dir.join("nte-updater.exe");
        fs::write(&updater_exe, b"updater").unwrap();
        fs::write(staging_dir.join("nte-dps-tool.exe"), b"app").unwrap();
        let tx_path = tx_dir.join(format!("{tx_id}.json"));
        fs::write(&tx_path, b"{}").unwrap();

        let base = UpdateTransaction {
            schema: UPDATE_SCHEMA_VERSION,
            id: tx_id.to_owned(),
            parent_pid: std::process::id(),
            install_dir: install_dir.clone(),
            staging_dir: staging_dir.clone(),
            health_marker: health_dir.join(format!("{tx_id}.ok")),
            files: vec![PathBuf::from("nte-dps-tool.exe")],
        };

        assert!(ValidatedTransaction::new(&tx_path, base.clone(), &updater_exe).is_ok());

        // Mismatched install_dir outside the updater's own root must be rejected.
        let mut wrong_install = base.clone();
        wrong_install.install_dir = other_dir.clone();
        assert!(matches!(
            ValidatedTransaction::new(&tx_path, wrong_install, &updater_exe),
            Err(InstallUpdateError::InvalidTransaction(_))
        ));

        // Health marker traversal or mismatched file name must be rejected.
        let mut traversal_health = base.clone();
        traversal_health.health_marker = health_dir.join("..").join("escape.ok");
        assert!(matches!(
            ValidatedTransaction::new(&tx_path, traversal_health, &updater_exe),
            Err(InstallUpdateError::InvalidTransaction(_))
        ));

        let mut wrong_health_name = base.clone();
        wrong_health_name.health_marker = health_dir.join("other.ok");
        assert!(matches!(
            ValidatedTransaction::new(&tx_path, wrong_health_name, &updater_exe),
            Err(InstallUpdateError::InvalidTransaction(_))
        ));

        // Transaction file with wrong name/location must be rejected.
        let wrong_tx_path = tx_dir.join("different.json");
        fs::write(&wrong_tx_path, b"{}").unwrap();
        assert!(matches!(
            ValidatedTransaction::new(&wrong_tx_path, base, &updater_exe),
            Err(InstallUpdateError::InvalidTransaction(_))
        ));

        fs::remove_dir_all(root).expect("test directory should be removed");
    }
}
