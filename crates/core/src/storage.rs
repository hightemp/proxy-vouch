//! Private local autosave and portable backups. Secrets never use the row-view IPC schema.
use crate::{
    export,
    model::{AppError, AppResult, CheckResult, CheckSettings, Status},
    parser::{self, ParsedRow, MAX_ROWS},
    session::{self, Entry, Session, SharedSession},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const MAX_BACKUP_BYTES: usize = 256 * 1024 * 1024;
const FORMAT: &str = "proxy-pulse-backup";

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub theme: String,
    pub check: CheckSettings,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            check: CheckSettings::default(),
        }
    }
}

impl Preferences {
    pub fn validate(&self) -> AppResult<()> {
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            return Err(AppError::new(
                "INVALID_SETTINGS",
                "Choose a supported appearance.",
            ));
        }
        self.check.validate()
    }

    pub fn migrate_legacy(bytes: &[u8]) -> AppResult<Self> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Legacy {
            theme: String,
            concurrency: usize,
            rate_limit: u32,
        }
        let legacy: Legacy = serde_json::from_slice(bytes).map_err(|_| invalid_backup())?;
        let preferences = Self {
            theme: legacy.theme,
            check: CheckSettings {
                concurrency: legacy.concurrency,
                rate_limit: legacy.rate_limit,
                ..CheckSettings::default()
            },
        };
        preferences.validate()?;
        Ok(preferences)
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupScope {
    Full,
    Proxies,
    Settings,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RestoreMode {
    Merge,
    Replace,
    Skip,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEntry {
    pub parsed: ParsedRow,
    pub status: Status,
    pub result: Option<CheckResult>,
    pub result_settings: Option<CheckSettings>,
}

impl StoredEntry {
    fn from_entry(entry: &Entry) -> Self {
        let interrupted = matches!(entry.status, Status::Queued | Status::Checking);
        Self {
            parsed: entry.parsed.clone(),
            status: if interrupted {
                Status::Cancelled
            } else {
                entry.status
            },
            result: if interrupted {
                None
            } else {
                entry.result.clone()
            },
            result_settings: if interrupted {
                None
            } else {
                entry.result.as_ref().map(|r| r.settings.clone())
            },
        }
    }

    fn into_entry(mut self) -> Entry {
        if let (Some(result), Some(settings)) = (&mut self.result, self.result_settings) {
            result.settings = settings;
        }
        if matches!(self.status, Status::Queued | Status::Checking) {
            self.status = Status::Cancelled;
            self.result = None;
        }
        Entry {
            id: 0,
            version: 1,
            parsed: self.parsed,
            status: self.status,
            result: self.result,
            revision: 0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    format: String,
    version: u32,
    created_at: String,
    pub preferences: Option<Preferences>,
    pub entries: Option<Vec<StoredEntry>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPreview {
    pub created_at: String,
    pub proxies: Option<usize>,
    pub invalid: usize,
    pub results: usize,
    pub has_settings: bool,
    pub has_credentials: bool,
}

#[derive(Serialize)]
pub struct RestoreResult {
    pub added: usize,
    pub skipped: usize,
}

fn invalid_backup() -> AppError {
    AppError::new(
        "INVALID_BACKUP",
        "This is not a valid ProxyVouch backup. Choose a file exported from Backup & restore.",
    )
}

impl Backup {
    pub fn capture(session: &Session, scope: BackupScope) -> Self {
        Self {
            format: FORMAT.into(),
            version: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
            preferences: (!matches!(scope, BackupScope::Proxies))
                .then(|| session.preferences.clone()),
            entries: (!matches!(scope, BackupScope::Settings)).then(|| {
                session
                    .entries
                    .iter()
                    .map(StoredEntry::from_entry)
                    .collect()
            }),
        }
    }

    pub fn decode(bytes: &[u8]) -> AppResult<Self> {
        if bytes.len() > MAX_BACKUP_BYTES {
            return Err(AppError::new(
                "BACKUP_TOO_LARGE",
                "Backups must not exceed 256 MiB.",
            ));
        }
        #[derive(Deserialize)]
        struct Header {
            format: String,
            version: u32,
        }
        let header: Header = serde_json::from_slice(bytes).map_err(|_| invalid_backup())?;
        if header.format != FORMAT {
            return Err(invalid_backup());
        }
        if header.version != 1 {
            return Err(AppError::new(
                "BACKUP_VERSION_UNSUPPORTED",
                "This backup uses an unsupported version. Update ProxyVouch before importing it.",
            ));
        }
        let backup: Self = serde_json::from_slice(bytes).map_err(|_| invalid_backup())?;
        backup.validate()?;
        Ok(backup)
    }

    pub fn read(path: &Path) -> AppResult<Self> {
        let file = File::open(path)
            .map_err(|_| AppError::new("FILE_READ_FAILED", "Could not open the backup file."))?;
        let mut bytes = Vec::new();
        file.take((MAX_BACKUP_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| AppError::new("FILE_READ_FAILED", "Could not read the backup file."))?;
        Self::decode(&bytes)
    }

    pub fn encode(&self) -> AppResult<String> {
        let text = serde_json::to_string(self).map_err(|_| invalid_backup())?;
        if text.len() > MAX_BACKUP_BYTES {
            return Err(AppError::new(
                "BACKUP_TOO_LARGE",
                "The workspace exceeds the 256 MiB storage limit. Export and remove some records.",
            ));
        }
        Ok(text)
    }

    fn validate(&self) -> AppResult<()> {
        if self.preferences.is_none() && self.entries.is_none() {
            return Err(invalid_backup());
        }
        if let Some(preferences) = &self.preferences {
            preferences.validate()?;
        }
        if let Some(entries) = &self.entries {
            if entries.len() > MAX_ROWS {
                return Err(AppError::new(
                    "TOO_MANY_ROWS",
                    "The list cannot exceed 100,000 records.",
                ));
            }
            for entry in entries {
                match (&entry.parsed.proxy, &entry.parsed.error) {
                    (Some(proxy), None) if entry.status != Status::Invalid => {
                        parser::validate_proxy(proxy)?
                    }
                    (None, Some(_))
                        if entry.status == Status::Invalid && entry.result.is_none() => {}
                    _ => return Err(invalid_backup()),
                }
                if entry
                    .parsed
                    .delimiter
                    .is_some_and(|d| ![b',', b';', b'\t'].contains(&d))
                {
                    return Err(invalid_backup());
                }
                if let Some(result) = &entry.result {
                    if result.status != entry.status || entry.result_settings.is_none() {
                        return Err(invalid_backup());
                    }
                    if let Some(settings) = &entry.result_settings {
                        settings.validate()?;
                    }
                } else if matches!(
                    entry.status,
                    Status::Working | Status::Failed | Status::Inconclusive
                ) {
                    return Err(invalid_backup());
                }
            }
        }
        Ok(())
    }

    pub fn preview(&self) -> BackupPreview {
        let entries = self.entries.as_deref().unwrap_or_default();
        BackupPreview {
            created_at: self.created_at.clone(),
            proxies: self.entries.as_ref().map(Vec::len),
            invalid: entries
                .iter()
                .filter(|e| e.status == Status::Invalid)
                .count(),
            results: entries.iter().filter(|e| e.result.is_some()).count(),
            has_settings: self.preferences.is_some(),
            has_credentials: entries.iter().any(|e| {
                e.parsed
                    .proxy
                    .as_ref()
                    .is_some_and(|p| p.credentials.is_some())
                    || e.parsed.proxy.is_none()
            }),
        }
    }

    pub fn apply(
        &self,
        session: &mut Session,
        mode: RestoreMode,
        settings: bool,
    ) -> AppResult<RestoreResult> {
        session.ensure_idle()?;
        self.validate()?;
        if matches!(mode, RestoreMode::Skip) && !settings {
            return Err(AppError::new(
                "EMPTY_SELECTION",
                "Choose proxies or settings to import.",
            ));
        }
        let preferences = if settings {
            Some(
                self.preferences
                    .as_ref()
                    .ok_or_else(invalid_backup)?
                    .clone(),
            )
        } else {
            None
        };
        let mut added = 0;
        let mut skipped = 0;
        let replacement = if matches!(mode, RestoreMode::Skip) {
            None
        } else {
            let incoming = self.entries.as_ref().ok_or_else(invalid_backup)?;
            let mut entries = if matches!(mode, RestoreMode::Merge) {
                session.entries.clone()
            } else {
                Vec::new()
            };
            let mut keys: HashSet<_> = entries
                .iter()
                .filter_map(|e| e.parsed.proxy.clone())
                .collect();
            let mut invalid: HashSet<_> = entries
                .iter()
                .filter(|e| e.parsed.proxy.is_none())
                .map(|e| e.parsed.raw.clone())
                .collect();
            for entry in incoming {
                let duplicate = entry.parsed.proxy.as_ref().map_or_else(
                    || !invalid.insert(entry.parsed.raw.clone()),
                    |p| !keys.insert(p.clone()),
                );
                if matches!(mode, RestoreMode::Merge) && duplicate {
                    skipped += 1;
                    continue;
                }
                entries.push(entry.clone().into_entry());
                added += 1;
            }
            if entries.len() > MAX_ROWS {
                return Err(AppError::new(
                    "TOO_MANY_ROWS",
                    "The combined list cannot exceed 100,000 records.",
                ));
            }
            Some(entries)
        };
        // Everything is validated before mutating either settings or the list.
        if let Some(preferences) = preferences {
            session.set_preferences(preferences)?;
        }
        if let Some(entries) = replacement {
            session.replace_entries(entries);
        }
        Ok(RestoreResult { added, skipped })
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    pub directory: String,
    pub saved_revision: Option<u64>,
    pub error: Option<AppError>,
    pub notice: Option<String>,
}

struct StoreState {
    // Held for the process lifetime so concurrent instances cannot overwrite data.
    _lock: Option<File>,
    blocked: bool,
    status: StorageStatus,
}

pub struct Store {
    directory: PathBuf,
    inner: Mutex<StoreState>,
}

impl Store {
    pub fn open(directory: PathBuf, legacy_path: Option<&Path>) -> (Self, Session) {
        let mut session = Session::default();
        let mut inner = StoreState {
            _lock: None,
            blocked: false,
            status: StorageStatus {
                directory: directory.to_string_lossy().into_owned(),
                saved_revision: None,
                error: None,
                notice: None,
            },
        };
        let initialize = (|| -> AppResult<()> {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
                builder.mode(0o700);
                builder.create(&directory).map_err(|_| storage_error())?;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                    .map_err(|_| storage_error())?;
            }
            #[cfg(not(unix))]
            builder.create(&directory).map_err(|_| storage_error())?;
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options
                .open(directory.join("workspace.lock"))
                .map_err(|_| storage_error())?;
            file.try_lock().map_err(|_| AppError::new("STORAGE_LOCKED", "Another ProxyVouch instance is using this folder. Close it and restart this window to enable saving."))?;
            inner._lock = Some(file);
            let path = directory.join("workspace.json");
            if path.try_exists().map_err(|_| storage_error())? {
                let backup = match Backup::read(&path) {
                    Ok(backup) => backup,
                    Err(error) => {
                        // Never overwrite files from a newer application or unreadable storage.
                        if !matches!(
                            error.code.as_str(),
                            "INVALID_BACKUP"
                                | "INVALID_SETTINGS"
                                | "INVALID_HOST"
                                | "INVALID_PORT"
                                | "UNSUPPORTED_AUTH"
                                | "INVALID_FORMAT"
                        ) {
                            return Err(error);
                        }
                        let recovered = Backup::read(&directory.join("workspace.previous.json"))?;
                        let damaged = directory.join(format!(
                            "workspace.damaged-{}.json",
                            chrono::Utc::now().timestamp_micros()
                        ));
                        fs::rename(&path, damaged).map_err(|_| storage_error())?;
                        inner.status.notice = Some("Recovered the previous saved workspace. The damaged file was preserved in the data folder.".into());
                        recovered
                    }
                };
                backup.apply(&mut session, RestoreMode::Replace, true)?;
            } else if directory.join("workspace.previous.json").is_file() {
                Backup::read(&directory.join("workspace.previous.json"))?.apply(
                    &mut session,
                    RestoreMode::Replace,
                    true,
                )?;
                inner.status.notice = Some(
                    "Recovered the previous saved workspace because the main file was missing."
                        .into(),
                );
            } else if let Some(path) = legacy_path.filter(|p| p.is_file()) {
                let bytes = fs::read(path).map_err(|_| storage_error())?;
                match Preferences::migrate_legacy(&bytes) {
                    Ok(preferences) => session.set_preferences(preferences)?,
                    Err(_) => inner.status.notice = Some("The old preferences file could not be read. Defaults were loaded; the original file was preserved.".into()),
                }
            }
            Ok(())
        })();
        if let Err(error) = initialize {
            inner.blocked = true;
            inner.status.error = Some(error);
        }
        (
            Self {
                directory,
                inner: Mutex::new(inner),
            },
            session,
        )
    }

    pub fn status(&self) -> AppResult<StorageStatus> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| storage_error())?
            .status
            .clone())
    }

    fn write(&self, backup: &Backup) -> AppResult<()> {
        let text = backup.encode()?;
        let path = self.directory.join("workspace.json");
        if path.try_exists().map_err(|_| storage_error())? {
            let previous = Backup::read(&path)?.encode()?;
            export::save_atomic(&self.directory.join("workspace.previous.json"), &previous)?;
        }
        export::save_atomic(&path, &text)
    }

    pub fn set_preferences(
        &self,
        shared: &SharedSession,
        preferences: Preferences,
    ) -> AppResult<()> {
        preferences.validate()?;
        let mut inner = self.inner.lock().map_err(|_| storage_error())?;
        if inner.blocked {
            return Err(inner.status.error.clone().unwrap_or_else(storage_error));
        }
        let mut session = session::lock(shared)?;
        let mut backup = Backup::capture(&session, BackupScope::Full);
        backup.preferences = Some(preferences.clone());
        if let Err(error) = self.write(&backup) {
            inner.status.error = Some(error.clone());
            return Err(error);
        }
        session.set_preferences(preferences)?;
        inner.status.saved_revision = Some(session.revision);
        inner.status.error = None;
        Ok(())
    }

    pub fn restore(
        &self,
        shared: &SharedSession,
        backup: &Backup,
        mode: RestoreMode,
        settings: bool,
    ) -> AppResult<RestoreResult> {
        let mut inner = self.inner.lock().map_err(|_| storage_error())?;
        if inner.blocked {
            return Err(inner.status.error.clone().unwrap_or_else(storage_error));
        }
        let mut session = session::lock(shared)?;
        session.ensure_idle()?;
        let mut candidate = Session::default();
        candidate.entries = session.entries.clone();
        candidate.preferences = session.preferences.clone();
        let result = backup.apply(&mut candidate, mode, settings)?;
        if let Err(error) = self.write(&Backup::capture(&candidate, BackupScope::Full)) {
            inner.status.error = Some(error.clone());
            return Err(error);
        }
        // The list remains unchanged if validation or disk writing fails.
        if settings {
            session.set_preferences(candidate.preferences)?;
        }
        if !matches!(mode, RestoreMode::Skip) {
            session.replace_entries(candidate.entries);
        }
        inner.status.saved_revision = Some(session.revision);
        inner.status.error = None;
        Ok(result)
    }

    pub fn save(&self, shared: &SharedSession) -> AppResult<()> {
        // One writer, including manual flushes. Capture after acquiring this lock
        // so an older background snapshot cannot replace a newer explicit save.
        let mut inner = self.inner.lock().map_err(|_| storage_error())?;
        if inner.blocked {
            return Err(inner.status.error.clone().unwrap_or_else(storage_error));
        }
        let (revision, backup) = {
            let session = session::lock(shared)?;
            if inner.status.saved_revision == Some(session.revision) && inner.status.error.is_none()
            {
                return Ok(());
            }
            (
                session.revision,
                Backup::capture(&session, BackupScope::Full),
            )
        };
        let result = self.write(&backup);
        match &result {
            Ok(()) => {
                inner.status.saved_revision = Some(revision);
                inner.status.error = None;
            }
            Err(error) => inner.status.error = Some(error.clone()),
        }
        result
    }
}

fn storage_error() -> AppError {
    AppError::new("STORAGE_ERROR", "Automatic saving failed. Check access to the data folder and available disk space. Your previous file was preserved.")
}
