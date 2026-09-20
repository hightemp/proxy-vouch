use proxy_vouch_core::{
    model::{
        AnonymityLevel, AnonymityResult, CheckResult, CheckSettings, Protocol, SpeedResult, Status,
        TransferLimit, TransferOutcome,
    },
    parser::ImportOptions,
    session::{Session, SharedSession},
    storage::{Backup, BackupScope, Preferences, RestoreMode, Store},
};
use std::{
    fs,
    sync::{Arc, Mutex},
};

fn list(text: &str) -> Session {
    let mut session = Session::default();
    session.preview(text, &ImportOptions::default()).unwrap();
    session.commit_import(false, true, true).unwrap();
    session
}

fn custom_preferences() -> Preferences {
    Preferences {
        theme: "dark".into(),
        check: CheckSettings {
            url: "https://check.example/ip?token=fixture-token".into(),
            fallback_url: "http://fallback.example/ip".into(),
            ip_echo: false,
            country_lookup: false,
            anonymity_check: false,
            speed_check: false,
            speed_test_mib: 4,
            expected_status: 201,
            body_contains: "accepted".into(),
            concurrency: 7,
            rate_limit: 3,
            connect_timeout_ms: 2000,
            attempt_timeout_ms: 4000,
            total_timeout_ms: 10000,
            retries: 1,
        },
    }
}

fn success(settings: CheckSettings) -> CheckResult {
    CheckResult {
        status: Status::Working,
        detected: Some(Protocol::Socks5h),
        authentication: "Authenticated".into(),
        latency_ms: Some(25),
        total_duration_ms: 30,
        exit_ip: Some("198.51.100.9".into()),
        country_code: Some("DE".into()),
        anonymity: Some(AnonymityResult {
            level: AnonymityLevel::Anonymous,
            message: "A proxy indicator was observed.".into(),
            check_url: "http://judge.example/get".into(),
            observed_ip: Some("198.51.100.9".into()),
            proxy_headers: vec!["via".into()],
        }),
        speed: Some(SpeedResult {
            outcome: TransferOutcome::Completed,
            download_mbps: Some(8.388608),
            requested_bytes: 1_048_576,
            received_bytes: 1_048_576,
            duration_ms: 1000,
            limit: TransferLimit::NotObserved,
            http_status: Some(200),
            proxy_http_status: None,
            check_url: "https://download.example/data".into(),
            message: "The selected sample was received in full; total traffic quota is unknown."
                .into(),
        }),
        checked_at: "2026-09-06T00:00:00Z".into(),
        code: String::new(),
        stage: "complete".into(),
        message: "The check request completed successfully.".into(),
        check_url: settings.safe_url(),
        attempts: Vec::new(),
        settings,
    }
}

#[test]
fn full_backup_preserves_credentials_formats_duplicates_and_invalid_rows() {
    let mut source = list("socks5h://demo%40user:p%3Aass%25@[2001:db8::1]:1080\nhttp://demo:@proxy.example:8080\nproxy.example:1080\nproxy.example:1080\nbad record");
    source
        .preview(
            "type;server;port;login;pass;label\nhttp;csv.example;8080;demo;\"p;a:ss\";Europe",
            &ImportOptions::default(),
        )
        .unwrap();
    source.commit_import(false, true, true).unwrap();
    source.set_preferences(custom_preferences()).unwrap();
    let encoded = Backup::capture(&source, BackupScope::Full)
        .encode()
        .unwrap();
    let mut restored = Session::default();
    Backup::decode(encoded.as_bytes())
        .unwrap()
        .apply(&mut restored, RestoreMode::Replace, true)
        .unwrap();
    let original = Backup::capture(&source, BackupScope::Full);
    let copy = Backup::capture(&restored, BackupScope::Full);
    assert_eq!(
        serde_json::to_value(original.entries).unwrap(),
        serde_json::to_value(copy.entries).unwrap()
    );
    assert_eq!(
        serde_json::to_value(source.preferences).unwrap(),
        serde_json::to_value(restored.preferences).unwrap()
    );
    assert!(restored
        .entries
        .windows(2)
        .all(|rows| rows[0].id < rows[1].id));
}

#[test]
fn last_results_keep_their_profile_and_incomplete_checks_do_not_resume() {
    let mut source = list("proxy.example:1080\nother.example:1080\nthird.example:1080");
    let profile = custom_preferences().check;
    source.entries[0].status = Status::Working;
    source.entries[0].result = Some(success(profile.clone()));
    source.entries[1].status = Status::Checking;
    source.entries[2].status = Status::Queued;
    let mut restored = Session::default();
    let backup = Backup::decode(
        Backup::capture(&source, BackupScope::Full)
            .encode()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    backup
        .apply(&mut restored, RestoreMode::Replace, true)
        .unwrap();
    assert!(!restored.running);
    assert_eq!(
        restored
            .entries
            .iter()
            .map(|e| e.status)
            .collect::<Vec<_>>(),
        vec![Status::Working, Status::Cancelled, Status::Cancelled]
    );
    assert_eq!(
        restored.entries[0].result.as_ref().unwrap().settings,
        profile
    );
    assert_eq!(
        restored.entries[0]
            .result
            .as_ref()
            .unwrap()
            .country_code
            .as_deref(),
        Some("DE")
    );
    assert!(!serde_json::to_string(&restored.snapshot(0))
        .unwrap()
        .contains("fixture-token"));
    let anonymity = restored.entries[0]
        .result
        .as_ref()
        .unwrap()
        .anonymity
        .as_ref()
        .unwrap();
    assert_eq!(anonymity.level, AnonymityLevel::Anonymous);
    assert_eq!(anonymity.proxy_headers, vec!["via"]);
    let speed = restored.entries[0]
        .result
        .as_ref()
        .unwrap()
        .speed
        .as_ref()
        .unwrap();
    assert_eq!(speed.download_mbps, Some(8.388608));
    assert_eq!(speed.received_bytes, 1_048_576);
    assert_eq!(speed.limit, TransferLimit::NotObserved);
}

#[test]
fn backups_without_optional_check_fields_load_with_detection_enabled() {
    let mut source = list("http://proxy.example:8080");
    source.entries[0].status = Status::Working;
    source.entries[0].result = Some(success(CheckSettings::default()));
    let mut json = serde_json::to_value(Backup::capture(&source, BackupScope::Full)).unwrap();
    fn remove_country_fields(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                object.remove("countryLookup");
                object.remove("countryCode");
                object.remove("anonymityCheck");
                object.remove("anonymity");
                object.remove("speedCheck");
                object.remove("speedTestMib");
                object.remove("speed");
                for value in object.values_mut() {
                    remove_country_fields(value);
                }
            }
            serde_json::Value::Array(array) => array.iter_mut().for_each(remove_country_fields),
            _ => (),
        }
    }
    remove_country_fields(&mut json);
    let mut restored = Session::default();
    Backup::decode(&serde_json::to_vec(&json).unwrap())
        .unwrap()
        .apply(&mut restored, RestoreMode::Replace, true)
        .unwrap();
    assert!(restored.preferences.check.country_lookup);
    assert!(restored.preferences.check.anonymity_check);
    let result = restored.entries[0].result.as_ref().unwrap();
    assert_eq!(result.status, Status::Working);
    assert!(result.settings.country_lookup);
    assert!(result.country_code.is_none());
    assert!(result.settings.anonymity_check);
    assert!(result.anonymity.is_none());
    assert!(restored.preferences.check.speed_check);
    assert_eq!(restored.preferences.check.speed_test_mib, 1);
    assert!(result.settings.speed_check);
    assert_eq!(result.settings.speed_test_mib, 1);
    assert!(result.speed.is_none());
}

#[test]
fn reports_include_anonymity_without_credentials() {
    use proxy_vouch_core::export::{render, ExportOptions};
    let mut source = list("http://user:fixture-password@proxy.example:8080");
    source.entries[0].status = Status::Working;
    source.entries[0].result = Some(success(CheckSettings::default()));
    for format in ["json", "csv"] {
        let report = render(
            &source.entries,
            &ExportOptions {
                scope: "All".into(),
                format: format.into(),
                credentials: false,
                ids: vec![],
            },
        )
        .unwrap()
        .text;
        assert!(report.contains("Anonymous"));
        assert!(report.contains("http://judge.example/get"));
        assert!(report.contains("via"));
        assert!(report.contains("8.388608"));
        assert!(report.contains("1048576"));
        assert!(report.contains("NotObserved"));
        assert!(report.contains("https://download.example/data"));
        assert!(!report.contains("fixture-password"));
    }
}

#[test]
fn speed_sample_size_is_bounded() {
    for size in [0, 33, u32::MAX] {
        assert!(CheckSettings {
            speed_test_mib: size,
            ..CheckSettings::default()
        }
        .validate()
        .is_err());
    }
    for size in [1, 4, 32] {
        assert!(CheckSettings {
            speed_test_mib: size,
            ..CheckSettings::default()
        }
        .validate()
        .is_ok());
    }
}

#[test]
fn selective_backups_only_restore_the_selected_parts() {
    let mut source = list("proxy.example:1080:user:fixture-password");
    source.set_preferences(custom_preferences()).unwrap();
    let settings = Backup::capture(&source, BackupScope::Settings);
    assert!(!settings.encode().unwrap().contains("fixture-password"));
    assert_eq!(settings.preview().proxies, None);
    let mut restored = list("keep.example:8888");
    settings
        .apply(&mut restored, RestoreMode::Skip, true)
        .unwrap();
    assert_eq!(
        restored.entries[0].parsed.proxy.as_ref().unwrap().host,
        "keep.example"
    );
    assert_eq!(restored.preferences.theme, "dark");
    let proxies = Backup::capture(&source, BackupScope::Proxies);
    assert!(!proxies.preview().has_settings);
    assert!(!proxies.encode().unwrap().contains("fixture-token"));
    assert!(!serde_json::to_string(&proxies.preview())
        .unwrap()
        .contains("fixture-password"));
}

#[test]
fn merging_deduplicates_exact_endpoints_and_keeps_existing_results() {
    let mut target = list("socks5://user:pass@proxy.example:1080");
    target.entries[0].status = Status::Working;
    target.entries[0].result = Some(success(CheckSettings::default()));
    let incoming = list("socks5://user:pass@proxy.example:1080\nsocks5h://user:pass@proxy.example:1080\nsocks5://user:other@proxy.example:1080\nbad\nbad");
    let result = Backup::capture(&incoming, BackupScope::Proxies)
        .apply(&mut target, RestoreMode::Merge, false)
        .unwrap();
    assert_eq!(
        (result.added, result.skipped, target.entries.len()),
        (3, 2, 4)
    );
    assert_eq!(target.entries[0].status, Status::Working);
    assert_eq!(
        target.entries[0].result.as_ref().unwrap().latency_ms,
        Some(25)
    );
}

#[test]
fn malformed_or_future_backups_and_reports_are_rejected_without_mutation() {
    let mut target = list("keep.example:8080");
    let original = target.entries[0].id;
    let backup = Backup::capture(&list("new.example:8888"), BackupScope::Full);
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("format", serde_json::json!("other")),
    ] {
        let mut json = serde_json::to_value(&backup).unwrap();
        json[field] = value;
        assert!(Backup::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    let mut invalid = backup.clone();
    invalid.preferences = Some(Preferences {
        theme: "bad-theme".into(),
        ..Preferences::default()
    });
    assert!(invalid
        .apply(&mut target, RestoreMode::Replace, true)
        .is_err());
    let mut invalid = backup;
    invalid.entries.as_mut().unwrap()[0]
        .parsed
        .proxy
        .as_mut()
        .unwrap()
        .port = 0;
    assert!(invalid
        .apply(&mut target, RestoreMode::Replace, true)
        .is_err());
    assert!(Backup::decode(br#"{"schema_version":1,"records":[]}"#).is_err());
    assert_eq!(target.entries.len(), 1);
    assert_eq!(target.entries[0].id, original);
    assert_eq!(target.preferences.theme, "system");
}

#[test]
fn restore_is_rejected_during_a_run() {
    let mut target = list("keep.example:8080");
    target.running = true;
    let error = Backup::capture(&Session::default(), BackupScope::Full)
        .apply(&mut target, RestoreMode::Replace, true)
        .err()
        .unwrap();
    assert_eq!(error.code, "CHECK_RUNNING");
    assert_eq!(target.entries.len(), 1);
}

#[test]
fn autosave_reopens_settings_and_list_and_persists_clearing() {
    let temp = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(temp.path().join("data"), None);
    let shared = Arc::new(Mutex::new(list("proxy.example:8080:user:fixture-password")));
    store
        .set_preferences(&shared, custom_preferences())
        .unwrap();
    drop(store);
    let (store, restored) = Store::open(temp.path().join("data"), None);
    assert_eq!(restored.entries.len(), 1);
    assert_eq!(restored.preferences.check, custom_preferences().check);
    let shared = Arc::new(Mutex::new(restored));
    shared.lock().unwrap().clear(&[]).unwrap();
    store.save(&shared).unwrap();
    drop(store);
    let (_, restored) = Store::open(temp.path().join("data"), None);
    assert!(restored.entries.is_empty());
}

fn two_generations(path: &std::path::Path) {
    let (store, _) = Store::open(path.into(), None);
    let shared = Arc::new(Mutex::new(list("first.example:8080")));
    store.save(&shared).unwrap();
    let id = shared.lock().unwrap().entries[0].id;
    shared
        .lock()
        .unwrap()
        .edit(id, "second.example:8080")
        .unwrap();
    store.save(&shared).unwrap();
}

#[test]
fn damaged_main_file_recovers_the_previous_generation_and_preserves_evidence() {
    let temp = tempfile::tempdir().unwrap();
    two_generations(temp.path());
    fs::write(temp.path().join("workspace.json"), b"broken").unwrap();
    let (store, restored) = Store::open(temp.path().into(), None);
    assert_eq!(
        restored.entries[0].parsed.proxy.as_ref().unwrap().host,
        "first.example"
    );
    assert!(store.status().unwrap().notice.is_some());
    let preserved = fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("workspace.damaged-")
        })
        .unwrap();
    assert_eq!(fs::read(preserved.path()).unwrap(), b"broken");
    store.save(&Arc::new(Mutex::new(restored))).unwrap();
    assert!(Backup::read(&temp.path().join("workspace.json")).is_ok());
}

#[test]
fn missing_main_file_recovers_the_previous_generation() {
    let temp = tempfile::tempdir().unwrap();
    two_generations(temp.path());
    fs::remove_file(temp.path().join("workspace.json")).unwrap();
    let (store, restored) = Store::open(temp.path().into(), None);
    assert!(store.status().unwrap().notice.is_some());
    assert_eq!(restored.entries.len(), 1);
}

#[test]
fn unreadable_workspace_is_never_replaced_by_an_empty_session() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("workspace.json"), b"broken").unwrap();
    let (store, session) = Store::open(temp.path().into(), None);
    assert!(store.status().unwrap().error.is_some());
    assert!(store.save(&Arc::new(Mutex::new(session))).is_err());
    assert_eq!(
        fs::read(temp.path().join("workspace.json")).unwrap(),
        b"broken"
    );
}

#[test]
fn a_second_instance_cannot_overwrite_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let (first, _) = Store::open(temp.path().into(), None);
    first
        .save(&Arc::new(Mutex::new(list("keep.example:8080"))))
        .unwrap();
    let (second, session) = Store::open(temp.path().into(), None);
    assert_eq!(
        second.status().unwrap().error.unwrap().code,
        "STORAGE_LOCKED"
    );
    assert!(second.save(&Arc::new(Mutex::new(session))).is_err());
    drop(first);
    let (third, restored) = Store::open(temp.path().into(), None);
    assert!(third.status().unwrap().error.is_none());
    assert_eq!(restored.entries.len(), 1);
}

#[test]
fn a_failed_write_preserves_disk_state_and_restore_is_transactional() {
    let temp = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(temp.path().into(), None);
    let shared: SharedSession = Arc::new(Mutex::new(list("keep.example:8080")));
    store.save(&shared).unwrap();
    let original = fs::read(temp.path().join("workspace.json")).unwrap();
    // A directory in the backup slot reliably rejects a replacement, even as root.
    fs::create_dir(temp.path().join("workspace.previous.json")).unwrap();
    assert!(store
        .set_preferences(&shared, custom_preferences())
        .is_err());
    assert_eq!(shared.lock().unwrap().preferences.theme, "system");
    let mut incoming = list("new.example:8080");
    incoming.set_preferences(custom_preferences()).unwrap();
    assert!(store
        .restore(
            &shared,
            &Backup::capture(&incoming, BackupScope::Full),
            RestoreMode::Replace,
            true
        )
        .is_err());
    assert_eq!(
        shared.lock().unwrap().entries[0]
            .parsed
            .proxy
            .as_ref()
            .unwrap()
            .host,
        "keep.example"
    );
    assert_eq!(shared.lock().unwrap().preferences.theme, "system");
    assert_eq!(
        fs::read(temp.path().join("workspace.json")).unwrap(),
        original
    );
    assert!(store.status().unwrap().error.is_some());
    fs::remove_dir(temp.path().join("workspace.previous.json")).unwrap();
    store.save(&shared).unwrap();
    assert!(store.status().unwrap().error.is_none());
    store
        .restore(
            &shared,
            &Backup::capture(&incoming, BackupScope::Full),
            RestoreMode::Replace,
            true,
        )
        .unwrap();
    assert!(store.status().unwrap().error.is_none());
    assert_eq!(shared.lock().unwrap().preferences.theme, "dark");
}

#[test]
fn legacy_preferences_migrate_without_changing_the_original() {
    let temp = tempfile::tempdir().unwrap();
    let legacy = temp.path().join("preferences.json");
    let text = br#"{"theme":"dark","concurrency":6,"rateLimit":4}"#;
    fs::write(&legacy, text).unwrap();
    let (_, restored) = Store::open(temp.path().join("data"), Some(&legacy));
    assert_eq!(restored.preferences.theme, "dark");
    assert_eq!(restored.preferences.check.concurrency, 6);
    assert_eq!(restored.preferences.check.rate_limit, 4);
    assert_eq!(fs::read(legacy).unwrap(), text);
}

#[test]
fn concurrent_flushes_cannot_replace_a_newer_saved_revision() {
    let temp = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(temp.path().into(), None);
    let store = Arc::new(store);
    let shared = Arc::new(Mutex::new(list("proxy.example:8080")));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let shared = Arc::clone(&shared);
            let store = Arc::clone(&store);
            scope.spawn(move || {
                for _ in 0..4 {
                    shared.lock().unwrap().revision += 1;
                    store.save(&shared).unwrap();
                }
            });
        }
    });
    assert_eq!(
        store.status().unwrap().saved_revision,
        Some(shared.lock().unwrap().revision)
    );
    assert_eq!(
        Backup::read(&temp.path().join("workspace.json"))
            .unwrap()
            .preview()
            .proxies,
        Some(1)
    );
}

#[cfg(unix)]
#[test]
fn saved_credentials_are_only_readable_by_the_owner() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data");
    two_generations(&directory);
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in [
        "workspace.json",
        "workspace.previous.json",
        "workspace.lock",
    ] {
        assert_eq!(
            fs::metadata(directory.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn combined_lists_cannot_exceed_the_row_limit_or_partially_import_settings() {
    let mut target = list("keep.example:8080");
    let entry = target.entries[0].clone();
    target.entries = vec![entry; proxy_vouch_core::parser::MAX_ROWS];
    let mut source = list("new.example:8080");
    source.set_preferences(custom_preferences()).unwrap();
    let error = Backup::capture(&source, BackupScope::Full)
        .apply(&mut target, RestoreMode::Merge, true)
        .err()
        .unwrap();
    assert_eq!(error.code, "TOO_MANY_ROWS");
    assert_eq!(target.preferences.theme, "system");
    assert_eq!(target.entries.len(), proxy_vouch_core::parser::MAX_ROWS);
}
