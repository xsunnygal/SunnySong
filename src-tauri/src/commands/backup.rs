use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;
use solmusic_application::SunnySongApp;
use solmusic_sqlite::{validate_restore_candidate, SqliteMusicRepository};
use tauri::State;
use uuid::Uuid;

const DATABASE_FILE: &str = "solmusic.sqlite3";
const PENDING_RESTORE_FILE: &str = "pending-restore.sqlite3";
const MAX_RESTORE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub database: PathBuf,
    pending_restore: PathBuf,
}

impl AppPaths {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            database: data_dir.join(DATABASE_FILE),
            pending_restore: data_dir.join(PENDING_RESTORE_FILE),
            data_dir,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub path: String,
    pub restart_required: bool,
    pub message: String,
}

#[tauri::command]
pub fn export_backup(
    app: State<'_, SunnySongApp>,
    paths: State<'_, AppPaths>,
    path: String,
) -> Result<BackupStatus, String> {
    ensure_desktop()?;
    let destination = validated_destination(&path, &paths)?;
    let temporary = sibling_temporary_path(&destination, "backup");
    let result = (|| {
        app.create_backup(&temporary).map_err(error)?;
        File::open(&temporary)
            .and_then(|file| file.sync_all())
            .map_err(|cause| format!("could not flush backup to disk: {cause}"))?;
        replace_file(&temporary, &destination)?;
        sync_parent(&destination)?;
        Ok(BackupStatus {
            path: display_path(&destination)?,
            restart_required: false,
            message: "Backup exported successfully. Credentials and cookies are not included."
                .into(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[tauri::command]
pub fn stage_backup_restore(
    paths: State<'_, AppPaths>,
    path: String,
) -> Result<BackupStatus, String> {
    ensure_desktop()?;
    let source = validated_restore_source(&path, &paths)?;
    validate_restore_candidate(&source).map_err(error)?;

    let temporary = sibling_temporary_path(&paths.pending_restore, "restore");
    let result = (|| {
        copy_limited(&source, &temporary, MAX_RESTORE_BYTES)?;
        validate_restore_candidate(&temporary).map_err(error)?;
        replace_file(&temporary, &paths.pending_restore)?;
        sync_parent(&paths.pending_restore)?;
        Ok(BackupStatus {
            path: display_path(&source)?,
            restart_required: true,
            message: "Restore validated and staged. Restart SunnySong to apply it. Credentials and cookies will remain unchanged."
                .into(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn apply_pending_restore(paths: &AppPaths) -> Result<bool, String> {
    let rollback_database = paths.data_dir.join("pre-restore.sqlite3");
    let live_files = database_family(&paths.database);
    let rollback_files = database_family(&rollback_database);
    if !paths.database.exists() && rollback_database.exists() {
        for live in &live_files {
            remove_if_exists(live)?;
        }
        rollback_moves(
            &live_files
                .iter()
                .cloned()
                .zip(rollback_files.iter().cloned())
                .collect::<Vec<_>>(),
        );
    }
    if !paths.pending_restore.exists() {
        if paths.database.exists() {
            for rollback in &rollback_files {
                remove_if_exists(rollback)?;
            }
        }
        return Ok(false);
    }
    validate_restore_candidate(&paths.pending_restore).map_err(error)?;
    for rollback in &rollback_files {
        remove_if_exists(rollback)?;
    }

    let mut moved = Vec::new();
    for (live, rollback) in live_files.iter().zip(&rollback_files) {
        if live.exists() {
            if let Err(cause) = fs::rename(live, rollback) {
                rollback_moves(&moved);
                return Err(format!(
                    "could not preserve current database before restore: {cause}"
                ));
            }
            moved.push((live.clone(), rollback.clone()));
        }
    }

    if let Err(cause) = fs::rename(&paths.pending_restore, &paths.database) {
        rollback_moves(&moved);
        return Err(format!("could not activate staged restore: {cause}"));
    }

    if let Err(cause) = SqliteMusicRepository::open(&paths.database) {
        for restored in &live_files {
            let _ = fs::remove_file(restored);
        }
        rollback_moves(&moved);
        return Err(format!(
            "restored database could not be opened; the previous database was kept: {cause}"
        ));
    }

    sync_parent(&paths.database)?;
    for rollback in rollback_files {
        let _ = remove_if_exists(&rollback);
    }
    Ok(true)
}

fn validated_destination(path: &str, paths: &AppPaths) -> Result<PathBuf, String> {
    let destination = nonblank_path(path, "Choose a backup destination")?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or("Backup destination must have a parent folder")?;
    if !parent.is_dir() {
        return Err("Backup destination folder does not exist".into());
    }
    if same_path(&destination, &paths.database) || same_path(&destination, &paths.pending_restore) {
        return Err(
            "Backup destination cannot be inside SunnySong's managed database files".into(),
        );
    }
    if destination.exists() && !destination.is_file() {
        return Err("Backup destination must be a regular file".into());
    }
    Ok(destination)
}

fn validated_restore_source(path: &str, paths: &AppPaths) -> Result<PathBuf, String> {
    let source = nonblank_path(path, "Choose a backup file")?;
    let metadata = fs::symlink_metadata(&source)
        .map_err(|cause| format!("Could not inspect backup file: {cause}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Restore source must be a regular file, not a link or folder".into());
    }
    if metadata.len() > MAX_RESTORE_BYTES {
        return Err("Backup is too large to restore safely (maximum 8 GiB)".into());
    }
    if same_path(&source, &paths.database) || same_path(&source, &paths.pending_restore) {
        return Err("Choose an exported backup, not SunnySong's live database".into());
    }
    Ok(source)
}

fn nonblank_path(value: &str, message: &str) -> Result<PathBuf, String> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(message.into());
    }
    Ok(PathBuf::from(value))
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn copy_limited(source: &Path, destination: &Path, maximum: u64) -> Result<(), String> {
    let mut input =
        File::open(source).map_err(|cause| format!("Could not open backup: {cause}"))?;
    let mut output = File::create_new(destination)
        .map_err(|cause| format!("Could not stage restore: {cause}"))?;
    let copied = io::copy(
        &mut std::io::Read::by_ref(&mut input).take(maximum + 1),
        &mut output,
    )
    .map_err(|cause| format!("Could not stage restore: {cause}"))?;
    if copied > maximum {
        return Err("Backup is too large to restore safely (maximum 8 GiB)".into());
    }
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|cause| format!("Could not flush staged restore: {cause}"))
}

fn replace_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    let old = sibling_temporary_path(destination, "previous");
    let had_destination = destination.exists();
    if had_destination {
        fs::rename(destination, &old)
            .map_err(|cause| format!("Could not replace existing file: {cause}"))?;
    }
    if let Err(cause) = fs::rename(temporary, destination) {
        if had_destination {
            let _ = fs::rename(&old, destination);
        }
        return Err(format!("Could not finalize file: {cause}"));
    }
    if had_destination {
        remove_if_exists(&old)?;
    }
    Ok(())
}

fn rollback_moves(moves: &[(PathBuf, PathBuf)]) {
    for (live, rollback) in moves.iter().rev() {
        let _ = fs::rename(rollback, live);
    }
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(format!("Could not remove {}: {cause}", path.display())),
    }
}

fn database_family(database: &Path) -> [PathBuf; 3] {
    [
        database.to_path_buf(),
        appended_path(database, "-wal"),
        appended_path(database, "-shm"),
    ]
}

fn appended_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value: OsString = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn sibling_temporary_path(destination: &Path, purpose: &str) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sunnysong-data");
    destination.with_file_name(format!(".{name}.{purpose}.{}.tmp", Uuid::new_v4()))
}

fn sync_parent(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let parent = path.parent().ok_or("Managed file has no parent folder")?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|cause| format!("Could not flush folder metadata: {cause}"))?;
    }
    Ok(())
}

fn display_path(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Selected path is not valid UTF-8".into())
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn ensure_desktop() -> Result<(), String> {
    if cfg!(desktop) {
        Ok(())
    } else {
        Err("Filesystem backup and restore are currently supported on desktop only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{appended_path, apply_pending_restore, copy_limited, AppPaths};
    use solmusic_application::{
        domain::{ArtistRef, Song, SongId},
        MusicRepository,
    };
    use solmusic_sqlite::SqliteMusicRepository;
    use std::{fs, path::PathBuf};

    #[test]
    fn database_sidecar_paths_append_without_replacing_extension() {
        let database = PathBuf::from("data/solmusic.sqlite3");
        assert_eq!(
            appended_path(&database, "-wal"),
            PathBuf::from("data/solmusic.sqlite3-wal")
        );
    }

    #[test]
    fn app_paths_keep_managed_files_together() {
        let paths = AppPaths::new(PathBuf::from("data"));
        assert_eq!(paths.database, PathBuf::from("data/solmusic.sqlite3"));
        assert_eq!(
            paths.pending_restore,
            PathBuf::from("data/pending-restore.sqlite3")
        );
    }

    #[test]
    fn applies_a_valid_staged_restore_before_repository_open() {
        let root =
            std::env::temp_dir().join(format!("sunnysong-restore-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let paths = AppPaths::new(root.clone());
        let song = |id: &str| Song {
            id: SongId::new(id).unwrap(),
            title: id.into(),
            artist: ArtistRef {
                id: None,
                name: "Artist".into(),
            },
            album_id: None,
            album_name: None,
            duration_ms: None,
            thumbnail_url: None,
        };
        let live = SqliteMusicRepository::open(&paths.database).unwrap();
        live.save_songs(&[song("old-track-1")]).unwrap();
        drop(live);
        let source = SqliteMusicRepository::open(root.join("source.sqlite3")).unwrap();
        source.save_songs(&[song("new-track-1")]).unwrap();
        source.create_backup(&paths.pending_restore).unwrap();
        drop(source);

        assert!(apply_pending_restore(&paths).unwrap());
        let restored = SqliteMusicRepository::open(&paths.database).unwrap();
        assert!(restored
            .song_by_id(&SongId::new("new-track-1").unwrap())
            .unwrap()
            .is_some());
        assert!(restored
            .song_by_id(&SongId::new("old-track-1").unwrap())
            .unwrap()
            .is_none());
        drop(restored);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn limited_copy_rejects_oversized_input() {
        let root =
            std::env::temp_dir().join(format!("sunnysong-copy-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"12345").unwrap();
        assert!(copy_limited(&source, &destination, 4).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
