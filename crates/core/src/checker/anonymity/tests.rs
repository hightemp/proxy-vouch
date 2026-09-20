use super::*;
use crate::{checker::check, model::Status, parser::parse_line};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{atomic::AtomicBool, Arc},
    thread::JoinHandle,
};

fn observation(origin: &str, headers: Value) -> Observation {
    let mut headers = headers.as_object().unwrap().clone();
    headers.insert(
        "User-Agent".into(),
        json!([concat!("ProxyVouch/", env!("CARGO_PKG_VERSION"))]),
    );
    parse(
        &serde_json::to_vec(&json!({
            "origin": origin, "headers": headers, "args": {"proxy_vouch_check": ["test"]},
        }))
        .unwrap(),
        "test",
    )
    .unwrap()
}

#[test]
fn classifies_disclosed_ip_proxy_indicators_and_judge_added_headers() {
    let baseline = observation(
        "198.51.100.1",
        json!({"Via":["1.1 judge"], "X-Forwarded-For": ["198.51.100.1"]}),
    );
    for (headers, expected) in [
        (
            json!({"Via":["1.1 judge"], "X-Forwarded-For":["203.0.113.2"]}),
            AnonymityLevel::Elite,
        ),
        (
            json!({"Via":["1.1 proxy, 1.1 judge"]}),
            AnonymityLevel::Anonymous,
        ),
        (
            json!({"x-forwarded-for":"198.51.100.1, 203.0.113.2"}),
            AnonymityLevel::Transparent,
        ),
        (
            json!({"Forwarded":"for=198.51.100.1:1234;proto=http"}),
            AnonymityLevel::Transparent,
        ),
        (
            json!({"x-client-ip":"::ffff:198.51.100.1"}),
            AnonymityLevel::Transparent,
        ),
        (
            json!({"Client-IP":"198.51.100.10"}),
            AnonymityLevel::Anonymous,
        ),
    ] {
        let observed = observation("203.0.113.2", headers);
        assert_eq!(classify(&baseline, &observed, JUDGE_URL).level, expected);
    }
    assert_eq!(
        classify(
            &baseline,
            &observation("198.51.100.1", json!({})),
            JUDGE_URL
        )
        .level,
        AnonymityLevel::Transparent
    );
}

#[test]
fn matches_ipv6_addresses_without_substring_false_positives() {
    let baseline = observation("2001:db8::1", json!({}));
    let observed = observation(
        "2001:db8::2",
        json!({"Forwarded":"for=\"[2001:0db8:0:0:0:0:0:1]:8181\""}),
    );
    assert_eq!(
        classify(&baseline, &observed, JUDGE_URL).level,
        AnonymityLevel::Transparent
    );
    let other = observation("2001:db8::2", json!({"Forwarded":"for=\"[2001:db8::10]\""}));
    assert_eq!(
        classify(&baseline, &other, JUDGE_URL).level,
        AnonymityLevel::Anonymous
    );
}

#[test]
fn different_ip_families_cannot_claim_elite_anonymity() {
    let baseline = observation("198.51.100.1", json!({}));
    let observed = observation("2001:db8::2", json!({}));
    assert_eq!(
        classify(&baseline, &observed, JUDGE_URL).level,
        AnonymityLevel::Unknown
    );
}

#[test]
fn incomplete_cached_and_malformed_responses_are_rejected() {
    for value in [
        json!({}),
        json!({"origin":"not an IP","headers":{},"args":{"proxy_vouch_check":["test"]}}),
        json!({"origin":"198.51.100.1","headers":{},"args":{"proxy_vouch_check":["test"]}}),
        json!({"origin":"198.51.100.1","headers":{"User-Agent":"unrelated"},"args":{"proxy_vouch_check":["test"]}}),
        json!({"origin":"198.51.100.1","headers":{"User-Agent":"test"},"args":{"proxy_vouch_check":["cached"]}}),
    ] {
        assert!(parse(&serde_json::to_vec(&value).unwrap(), "test").is_err());
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Elite,
    Anonymous,
    Transparent,
    Invalid,
    HttpError,
    Cancel,
    Stall,
}

struct Fixture {
    proxy: Proxy,
    control: Arc<Control>,
    requests: Arc<Mutex<Vec<(bool, String)>>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Fixture {
    fn new(mode: Mode, baseline_ok: bool) -> Self {
        let direct = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_server = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/get", direct.local_addr().unwrap());
        let proxy = parse_line(
            &format!(
                "http://demo:fixture-password@{}",
                proxy_server.local_addr().unwrap()
            ),
            false,
        )
        .unwrap();
        let control = Arc::new(Control {
            anonymity: Lookup::new(&url),
            ..Control::default()
        });
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let threads = [(false, direct), (true, proxy_server)].into_iter().map(|(proxied, listener)| {
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::clone(&stop);
            let requests = Arc::clone(&requests);
            let control = Arc::clone(&control);
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(stream) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        },
                        Err(error) => panic!("Fixture accept: {error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut bytes = Vec::new();
                    while !bytes.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        if stream.read_exact(&mut byte).is_err() { break; }
                        bytes.push(byte[0]);
                        assert!(bytes.len() <= 32768);
                    }
                    let request = String::from_utf8(bytes).unwrap();
                    let lookup = request.contains("proxy_vouch_check=");
                    requests.lock().unwrap().push((proxied, request.clone()));
                    if proxied && lookup && matches!(mode, Mode::Cancel | Mode::Stall) {
                        if matches!(mode, Mode::Cancel) { control.cancel(); }
                        while !stop.load(Ordering::SeqCst) { thread::sleep(Duration::from_millis(2)); }
                        continue;
                    }
                    let (status, body) = if !lookup {
                        (200, r#"{"ip":"203.0.113.2"}"#.to_owned())
                    } else if (!proxied && !baseline_ok) || (proxied && matches!(mode, Mode::HttpError)) {
                        (503, "unavailable".into())
                    } else if proxied && matches!(mode, Mode::Invalid) {
                        (200, "{}".into())
                    } else {
                        let target = request.split_whitespace().nth(1).unwrap();
                        let target = if target.starts_with('/') { format!("http://local{target}") } else { target.to_owned() };
                        let nonce = url::Url::parse(&target).unwrap().query_pairs()
                            .find(|(name, _)| name == "proxy_vouch_check").unwrap().1.into_owned();
                        let origin = if proxied { "203.0.113.2" } else { "198.51.100.1" };
                        let mut headers = json!({
                            "User-Agent": [concat!("ProxyVouch/", env!("CARGO_PKG_VERSION"))],
                            "Via": ["1.1 judge"], "X-Forwarded-For": [origin],
                        });
                        if proxied && matches!(mode, Mode::Anonymous) { headers["Via"] = json!(["1.1 proxy, 1.1 judge"]); }
                        if proxied && matches!(mode, Mode::Transparent) { headers["X-Forwarded-For"] = json!(["198.51.100.1, 203.0.113.2"]); }
                        (200, json!({"origin":origin,"headers":headers,"args":{"proxy_vouch_check":[nonce]}}).to_string())
                    };
                    let _ = write!(stream, "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }
            })
        }).collect();
        Self {
            proxy,
            control,
            requests,
            stop,
            threads,
        }
    }

    fn check(&self, enabled: bool) -> crate::model::CheckResult {
        check(
            &self.proxy,
            &CheckSettings {
                url: "http://check.invalid/".into(),
                country_lookup: false,
                anonymity_check: enabled,
                rate_limit: 100,
                ..CheckSettings::default()
            },
            &self.control,
            None,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[test]
fn checks_anonymity_through_the_authenticated_proxy_without_changing_availability() {
    for (mode, expected) in [
        (Mode::Elite, AnonymityLevel::Elite),
        (Mode::Anonymous, AnonymityLevel::Anonymous),
        (Mode::Transparent, AnonymityLevel::Transparent),
    ] {
        let fixture = Fixture::new(mode, true);
        let checked = fixture.check(true);
        assert_eq!(checked.status, Status::Working);
        assert_eq!(checked.anonymity.unwrap().level, expected);
        assert_eq!(checked.latency_ms, Some(checked.attempts[0].duration_ms));
        assert_eq!(checked.attempts.len(), 1);
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for (proxied, request) in requests.iter() {
            assert_eq!(request.contains("Proxy-Authorization: Basic "), *proxied);
        }
    }
}

#[test]
fn disabled_anonymity_sends_neither_a_direct_nor_a_proxied_lookup() {
    let fixture = Fixture::new(Mode::Elite, true);
    assert!(fixture.check(false).anonymity.is_none());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].1.contains("proxy_vouch_check="));
}

#[test]
fn concurrent_checks_share_one_direct_reference() {
    let fixture = Fixture::new(Mode::Elite, true);
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                assert_eq!(
                    fixture.check(true).anonymity.unwrap().level,
                    AnonymityLevel::Elite
                )
            });
        }
    });
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|(proxied, _)| !proxied).count(), 1);
}

#[test]
fn failed_reference_is_cached_and_returns_unknown_for_working_proxies() {
    let fixture = Fixture::new(Mode::Elite, false);
    for _ in 0..2 {
        let result = fixture.check(true);
        assert_eq!(result.status, Status::Working);
        assert_eq!(result.anonymity.unwrap().level, AnonymityLevel::Unknown);
    }
    assert_eq!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(proxied, _)| !proxied)
            .count(),
        1
    );
}

#[test]
fn invalid_unavailable_and_cancelled_lookups_preserve_working_results() {
    for mode in [Mode::Invalid, Mode::HttpError, Mode::Cancel] {
        let fixture = Fixture::new(mode, true);
        let result = fixture.check(true);
        assert_eq!(result.status, Status::Working);
        assert_eq!(result.anonymity.unwrap().level, AnonymityLevel::Unknown);
    }
}

#[test]
fn a_hanging_lookup_obeys_the_remaining_deadline() {
    let fixture = Fixture::new(Mode::Stall, true);
    let started = Instant::now();
    let result = fixture.control.anonymity.check(
        &fixture.proxy,
        Protocol::Http,
        &CheckSettings::default(),
        &fixture.control,
        started + Duration::from_millis(300),
    );
    assert_eq!(result.level, AnonymityLevel::Unknown);
    assert!(started.elapsed() < Duration::from_secs(1));
}
