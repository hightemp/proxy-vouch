use proptest::prelude::*;
use proxy_vouch_core::{
    export::{proxy_url, render, save_atomic, ExportOptions},
    model::{Protocol, Status},
    parser::{parse_import, parse_line, ImportOptions},
    session::Session,
};

#[test]
fn supported_text_formats_preserve_the_endpoint_and_protocol() {
    let cases = [
        ("192.0.2.10:8080", Protocol::Auto),
        ("192.0.2.10:8080:user:pass", Protocol::Auto),
        ("user:pass@192.0.2.10:8080", Protocol::Auto),
        ("http://192.0.2.10:8080", Protocol::Http),
        ("socks5://user:pass@192.0.2.10:8080", Protocol::Socks5),
        ("192.0.2.10:8080 socks5", Protocol::Socks5),
        ("192.0.2.10:8080:user:pass socks5", Protocol::Socks5),
        ("https 192.0.2.10:8080", Protocol::Https),
        ("socks5 192.0.2.10:8080:user:pass", Protocol::Socks5),
        ("user:pass@192.0.2.10:8080 socks5", Protocol::Socks5),
        ("socks4://192.0.2.10:8080", Protocol::Socks4),
        ("socks4a://userid@192.0.2.10:8080", Protocol::Socks4a),
        ("192.0.2.10 8080 user pass socks5", Protocol::Socks5),
        ("HTTP://192.0.2.10:8080/ http", Protocol::Http),
    ];
    for (input, protocol) in cases {
        let parsed = parse_line(input, false).unwrap_or_else(|err| panic!("{input}: {err}"));
        assert_eq!(
            (parsed.host.as_str(), parsed.port, parsed.protocol),
            ("192.0.2.10", 8080, protocol),
            "{input}"
        );
    }
}

#[test]
fn ipv6_and_percent_encoding_are_lossless() {
    let proxy = parse_line(
        "socks5h://demo%40user:p%3Aa%25ss%2B@[2001:db8::10]:1080",
        false,
    )
    .unwrap();
    assert_eq!(proxy.host, "2001:db8::10");
    let auth = proxy.credentials.unwrap();
    assert_eq!(auth.username, "demo@user");
    assert_eq!(auth.password.as_deref(), Some("p:a%ss+"));
    for value in [
        "[2001:db8::10]:1080",
        "[2001:db8::10]:1080:user:pass socks5",
    ] {
        assert!(parse_line(value, false).is_ok());
    }
}

#[test]
fn malformed_records_do_not_silently_change_meaning() {
    for input in [
        "proxy.example",
        "proxy.example:0",
        "proxy.example:65536",
        "proxy.example:abc",
        "2001:db8::10:1080",
        "http://proxy.example:8080 socks5",
        "auto://proxy.example:8080",
        "http://proxy.example",
        "http://proxy.example:8080/path",
        "http://proxy.example:8080?x=1",
        "http://user:p%ZZ@proxy.example:8080",
        "socks4://user:pass@proxy.example:1080",
        "socks5://user:@proxy.example:1080",
        "socks5://user:pass%00@proxy.example:1080",
        "http://proxy.example:8080:user:pass",
        "256.1.1.1:8080",
        "foo_bar.example:80",
    ] {
        assert!(
            parse_line(input, false).is_err(),
            "unexpected acceptance: {input}"
        );
    }
    assert!(parse_line("user:pass:proxy.example:1080", true).is_ok());
    assert!(parse_line("user:pass:proxy.example:1080", false).is_err());
}

#[test]
fn compact_percent_signs_and_empty_http_passwords_are_not_rewritten() {
    let proxy = parse_line("proxy.example:8080:user:p%40ss", false).unwrap();
    assert_eq!(
        proxy.credentials.unwrap().password.as_deref(),
        Some("p%40ss")
    );
    let proxy = parse_line("http://user:@proxy.example:8080", false).unwrap();
    assert_eq!(proxy.credentials.unwrap().password.as_deref(), Some(""));
}

#[test]
fn csv_mapping_and_quotes_preserve_credentials() {
    let input = "type;server;port;login;pass\r\nsocks5;proxy.example;1080;demo;\"p;a:ss@x\"\r\n";
    let parsed = parse_import(input, &ImportOptions::default()).unwrap();
    let proxy = parsed.rows[0].proxy.as_ref().unwrap();
    assert_eq!(
        proxy.credentials.as_ref().unwrap().password.as_deref(),
        Some("p;a:ss@x")
    );
    assert_eq!(parsed.rows[0].line, 2);
    let options = ImportOptions {
        format: "csv".into(),
        columns: vec![
            "protocol".into(),
            "host".into(),
            "port".into(),
            "username".into(),
            "password".into(),
        ],
        ..ImportOptions::default()
    };
    assert!(
        parse_import("socks5,proxy.example,1080,demo,pass", &options)
            .unwrap()
            .rows[0]
            .proxy
            .is_some()
    );
    let tsv = "host\tport\tprotocol\nproxy.example\t1080\tsocks5h";
    assert_eq!(
        parse_import(tsv, &ImportOptions::default()).unwrap().rows[0]
            .proxy
            .as_ref()
            .unwrap()
            .protocol,
        Protocol::Socks5h
    );
}

#[test]
fn malformed_csv_cannot_consume_or_rewrite_later_records() {
    let options = ImportOptions {
        format: "csv".into(),
        ..ImportOptions::default()
    };
    let parsed=parse_import("host,port,username,password,protocol\nproxy.example,1080,user,\"unclosed,socks5\nproxy.example,1081,user,good,socks5\nproxy.example,1082,user,bad\"quote,socks5",&options).unwrap();
    assert_eq!(parsed.rows.len(), 3);
    assert!(parsed.rows[0].error.is_some());
    assert_eq!(parsed.rows[1].proxy.as_ref().unwrap().port, 1081);
    assert!(parsed.rows[2].error.is_some());
}

#[test]
fn original_table_export_keeps_an_explicit_column_mapping() {
    let options = ImportOptions {
        format: "csv".into(),
        columns: vec![
            "protocol".into(),
            "host".into(),
            "port".into(),
            "username".into(),
            "password".into(),
        ],
        ..ImportOptions::default()
    };
    let mut state = Session::default();
    state
        .preview("socks5,proxy.example,1080,demo,pass", &options)
        .unwrap();
    state.commit_import(false, false, false).unwrap();
    let output = render(
        &state.entries,
        &ExportOptions {
            scope: "All".into(),
            format: "original".into(),
            credentials: true,
            ids: vec![],
        },
    )
    .unwrap();
    let reparsed = parse_import(&output.text, &ImportOptions::default()).unwrap();
    assert!(reparsed.rows[0].proxy == state.entries[0].parsed.proxy);
}

#[test]
fn input_limits_and_socks5_byte_limits_are_enforced() {
    assert!(parse_line(&"x".repeat(8193), false).is_err());
    assert!(parse_import(&"x".repeat(20 * 1024 * 1024 + 1), &ImportOptions::default()).is_err());
    let accepted = format!("socks5://user:{}@proxy.example:1080", "p".repeat(255));
    let rejected = format!("socks5://user:{}@proxy.example:1080", "p".repeat(256));
    assert!(parse_line(&accepted, false).is_ok());
    assert!(parse_line(&rejected, false).is_err());
}

#[test]
fn comments_bom_and_bad_rows_are_independent() {
    let parsed=parse_import("\u{feff}# comment\r\n\r\n// ignored\r\nproxy.example:80\r\ninvalid\r\nproxy.example:81:user:p#ass",&ImportOptions::default()).unwrap();
    assert_eq!(parsed.ignored, 3);
    assert_eq!(parsed.rows.len(), 3);
    assert!(parsed.rows[0].proxy.is_some());
    assert!(parsed.rows[1].error.is_some());
    assert_eq!(
        parsed.rows[2]
            .proxy
            .as_ref()
            .unwrap()
            .credentials
            .as_ref()
            .unwrap()
            .password
            .as_deref(),
        Some("p#ass")
    );
}

fn session(input: &str) -> Session {
    let mut session = Session::default();
    session.preview(input, &ImportOptions::default()).unwrap();
    session.commit_import(false, false, true).unwrap();
    session
}

#[test]
fn deduplication_respects_password_protocol_and_dns() {
    let mut state=session("socks5://user:one@proxy.example:1080\nsocks5://user:two@proxy.example:1080\nsocks5h://user:one@proxy.example:1080\nproxy.example:1080:user:one\nsocks5://user:one@proxy.example:1080");
    assert_eq!(state.entries.len(), 4);
    state
        .preview(
            "socks5://user:one@proxy.example:1080",
            &ImportOptions::default(),
        )
        .unwrap();
    assert_eq!(state.commit_import(false, false, true).unwrap(), 0);
    assert_eq!(state.commit_import(true, false, true).unwrap(), 1);
}

#[test]
fn export_groups_and_unknown_protocols_are_explicit() {
    let mut state = session("http://proxy.example:80\nproxy.example:81\nproxy.example:82\ninvalid");
    state.entries[0].status = Status::Working;
    state.entries[1].status = Status::Failed;
    state.entries[2].status = Status::Inconclusive;
    let mut options = ExportOptions {
        scope: "Failed".into(),
        format: "original".into(),
        credentials: true,
        ids: vec![],
    };
    assert_eq!(
        render(&state.entries, &options).unwrap().text,
        "proxy.example:81\n"
    );
    options.scope = "Checked".into();
    assert_eq!(render(&state.entries, &options).unwrap().count, 3);
    options.scope = "Failed".into();
    options.format = "urls".into();
    assert_eq!(
        render(&state.entries, &options).err().unwrap().code,
        "UNKNOWN_PROTOCOL"
    );
    options.scope = "Working".into();
    assert_eq!(render(&state.entries, &options).unwrap().count, 1);
}

#[test]
fn reports_redact_credentials_and_csv_formulas() {
    let state = session("http://user:unique-secret@proxy.example:8080");
    let mut options = ExportOptions {
        scope: "All".into(),
        format: "json".into(),
        credentials: false,
        ids: vec![],
    };
    assert!(!render(&state.entries, &options)
        .unwrap()
        .text
        .contains("unique-secret"));
    options.credentials = true;
    assert!(render(&state.entries, &options)
        .unwrap()
        .text
        .contains("unique-secret"));
    let mut state = session("http://%3Dformula:pass@proxy.example:8080");
    state.entries[0].parsed.label = " =CMD()".into();
    options.format = "csv".into();
    let csv = render(&state.entries, &options).unwrap();
    assert!(csv.text.contains("'=formula"));
    assert!(csv.text.contains("' =CMD()"));
}

#[test]
fn snapshots_never_include_raw_records_or_passwords() {
    let state = session("http://user:unique-secret@proxy.example:8080");
    let json = serde_json::to_string(&state.snapshot(0)).unwrap();
    assert!(!json.contains("unique-secret"));
    assert!(!json.contains("raw"));
}

#[test]
fn edits_reset_results_and_invalid_rows_can_be_fixed() {
    let mut state = session("invalid");
    let id = state.entries[0].id;
    state.edit(id, "socks5://proxy.example:1080").unwrap();
    assert_eq!(state.entries[0].status, Status::Unchecked);
    assert!(state.entries[0].parsed.error.is_none());
}

#[test]
fn atomic_save_preserves_existing_file_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("list.txt");
    save_atomic(&path, "first").unwrap();
    save_atomic(&path, "second").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    assert!(save_atomic(&dir.path().join("missing/list.txt"), "third").is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "second");
}

proptest! {
    #[test]
    fn uri_export_round_trip_preserves_special_credentials(username in "[a-zA-Z0-9@%+_-]{1,24}", password in "[a-zA-Z0-9@%+: /_-]{1,32}") {
        let mut state=session("socks5h://user:pass@[2001:db8::10]:1080");
        let auth=state.entries[0].parsed.proxy.as_mut().unwrap().credentials.as_mut().unwrap(); auth.username=username; auth.password=Some(password);
        let encoded=proxy_url(&state.entries[0],true).unwrap(); let parsed=parse_line(&encoded,false).unwrap();
        prop_assert!(parsed == *state.entries[0].parsed.proxy.as_ref().unwrap());
    }
    #[test]
    fn arbitrary_input_never_panics(text in ".{0,500}") { let _=parse_line(&text,false); }
}
