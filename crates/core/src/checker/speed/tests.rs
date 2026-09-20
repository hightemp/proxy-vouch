use super::*;
use crate::{checker::check, model::Status, parser::parse_line};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        Arc,
    },
    thread::JoinHandle,
};

#[derive(Clone, Copy)]
enum Mode {
    Complete,
    Short,
    WrongType,
    Encoded,
    Oversized,
    EndpointLimit,
    ProxyLimit,
    Stall,
    Cancel,
}

#[derive(Default)]
struct Stats {
    requests: Mutex<Vec<String>>,
    active: AtomicUsize,
    peak: AtomicUsize,
}

struct Active {
    stats: Arc<Stats>,
    finished: bool,
}

impl Active {
    fn new(stats: &Arc<Stats>) -> Self {
        let active = stats.active.fetch_add(1, Ordering::SeqCst) + 1;
        stats.peak.fetch_max(active, Ordering::SeqCst);
        Self {
            stats: Arc::clone(stats),
            finished: false,
        }
    }
    fn finish(&mut self) {
        if !self.finished {
            self.stats.active.fetch_sub(1, Ordering::SeqCst);
            self.finished = true;
        }
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.finish();
    }
}

struct Fixture {
    proxy: Proxy,
    control: Arc<Control>,
    stop: Arc<AtomicBool>,
    stats: Arc<Stats>,
    thread: Option<JoinHandle<()>>,
}

impl Fixture {
    fn new(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let proxy = parse_line(
            &format!(
                "http://demo:fixture-password@{}",
                listener.local_addr().unwrap()
            ),
            false,
        )
        .unwrap();
        let url = if matches!(mode, Mode::ProxyLimit) {
            "https://download.invalid/data"
        } else {
            "http://download.invalid/data"
        };
        let control = Arc::new(Control {
            speed: Check::new(url),
            ..Control::default()
        });
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats::default());
        let server_control = Arc::clone(&control);
        let server_stop = Arc::clone(&stop);
        let server_stats = Arc::clone(&stats);
        let thread = thread::spawn(move || {
            thread::scope(|scope| {
                while !server_stop.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(stream) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("Fixture accept: {error}"),
                    };
                    let control = &server_control;
                    let stop = &server_stop;
                    let stats = &server_stats;
                    scope.spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        if stream.read_exact(&mut byte).is_err() { return; }
                        request.push(byte[0]);
                        assert!(request.len() < 32768);
                    }
                    let request = String::from_utf8(request).unwrap();
                    stats.requests.lock().unwrap().push(request.clone());
                    if request.starts_with("GET http://check.invalid/") {
                        let body = r#"{"ip":"203.0.113.2"}"#;
                        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        return;
                    }
                    if matches!(mode, Mode::ProxyLimit | Mode::EndpointLimit) {
                        let code = if matches!(mode, Mode::ProxyLimit) { 509 } else { 429 };
                        let _ = write!(stream, "HTTP/1.1 {code} Limited\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        return;
                    }
                    let target = request.split_whitespace().nth(1).unwrap();
                    let bytes: usize = url::Url::parse(target).unwrap().query_pairs()
                        .find(|(name, _)| name == "bytes").unwrap().1.parse().unwrap();
                    let length = if matches!(mode, Mode::Oversized) { bytes * 2 } else { bytes };
                    let content_type = if matches!(mode, Mode::WrongType) { "text/html" } else { "application/octet-stream" };
                    let encoding = if matches!(mode, Mode::Encoded) { "Content-Encoding: gzip\r\n" } else { "" };
                    if write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\n{encoding}Connection: close\r\n\r\n").is_err() { return; }
                    let mut active = Active::new(stats);
                    let amount = if matches!(mode, Mode::Short) { bytes / 2 } else { length };
                    let chunk = [b'x'; 65536];
                    let mut sent = 0;
                    while sent < amount && !stop.load(Ordering::SeqCst) {
                        if sent > 0 && matches!(mode, Mode::Cancel | Mode::Stall) {
                            if matches!(mode, Mode::Cancel) { control.cancel(); }
                            while !stop.load(Ordering::SeqCst) { thread::sleep(Duration::from_millis(2)); }
                            return;
                        }
                        let next = chunk.len().min(amount - sent);
                        // Do not count teardown after the final byte as overlap.
                        if sent + next == amount { active.finish(); }
                        if stream.write_all(&chunk[..next]).is_err() { return; }
                        sent += next;
                        thread::sleep(Duration::from_millis(3));
                    }
                });
                }
            })
        });
        Self {
            proxy,
            control,
            stop,
            stats,
            thread: Some(thread),
        }
    }

    fn check(&self, enabled: bool, sample_mib: u32) -> crate::model::CheckResult {
        check(
            &self.proxy,
            &CheckSettings {
                url: "http://check.invalid/".into(),
                country_lookup: false,
                anonymity_check: false,
                speed_check: enabled,
                speed_test_mib: sample_mib,
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
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn measures_the_selected_sample_through_the_authenticated_proxy() {
    for size in [1, 2] {
        let fixture = Fixture::new(Mode::Complete);
        let result = fixture.check(true, size);
        assert_eq!(result.status, Status::Working);
        assert_eq!(result.latency_ms, Some(result.attempts[0].duration_ms));
        assert_eq!(result.attempts.len(), 1);
        let speed = result.speed.unwrap();
        assert_eq!(speed.outcome, TransferOutcome::Completed);
        assert_eq!(speed.received_bytes, u64::from(size) * 1024 * 1024);
        assert_eq!(speed.requested_bytes, speed.received_bytes);
        assert_eq!(speed.limit, TransferLimit::NotObserved);
        let from_millis = speed.received_bytes as f64 * 8.0 / speed.duration_ms as f64 / 1000.0;
        assert!((speed.download_mbps.unwrap() / from_millis - 1.0).abs() < 0.05);
        assert!(speed.message.contains("quota"));
        let requests = fixture.stats.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("GET http://download.invalid/data?bytes="));
        assert!(requests[1].contains("Proxy-Authorization: Basic "));
        assert!(requests[1].contains("Accept-Encoding: identity"));
    }
}

#[test]
fn disabled_speed_checks_make_no_extra_request() {
    let fixture = Fixture::new(Mode::Complete);
    assert!(fixture.check(false, 1).speed.is_none());
    assert_eq!(fixture.stats.requests.lock().unwrap().len(), 1);
}

#[test]
fn invalid_html_compressed_and_oversized_responses_are_not_speed_samples() {
    for mode in [Mode::WrongType, Mode::Encoded, Mode::Oversized] {
        let fixture = Fixture::new(mode);
        let result = fixture.check(true, 1);
        assert_eq!(result.status, Status::Working);
        let speed = result.speed.unwrap();
        assert_eq!(speed.outcome, TransferOutcome::Failed);
        assert_eq!(speed.limit, TransferLimit::Unknown);
        assert!(speed.download_mbps.is_none());
        assert!(speed.received_bytes <= speed.requested_bytes);
    }
}

#[test]
fn limit_responses_identify_proxy_and_endpoint_without_inventing_a_quota() {
    for (mode, code, source) in [
        (Mode::ProxyLimit, 509, "proxy"),
        (Mode::EndpointLimit, 429, "download endpoint"),
    ] {
        let fixture = Fixture::new(mode);
        let result = fixture.check(true, 1);
        assert_eq!(result.status, Status::Working);
        let speed = result.speed.unwrap();
        assert_eq!(speed.limit, TransferLimit::Signaled);
        assert!(speed.message.contains(source));
        assert!(speed.message.contains("does not establish"));
        assert!(speed.download_mbps.is_none());
        if source == "proxy" {
            assert_eq!(speed.proxy_http_status, Some(code));
        } else {
            assert_eq!(speed.http_status, Some(code));
        }
    }
}

#[test]
fn short_transfers_record_partial_speed_but_do_not_claim_a_traffic_limit() {
    let fixture = Fixture::new(Mode::Short);
    let result = fixture.check(true, 1);
    assert_eq!(result.status, Status::Working);
    let speed = result.speed.unwrap();
    assert_eq!(speed.outcome, TransferOutcome::Partial);
    assert_eq!(speed.limit, TransferLimit::Unknown);
    assert_eq!(speed.received_bytes, 524288);
    assert!(speed.download_mbps.unwrap() > 0.0);
}

#[test]
fn cancellation_and_deadlines_stop_downloads_without_failing_the_proxy() {
    let fixture = Fixture::new(Mode::Cancel);
    let result = fixture.check(true, 1);
    assert_eq!(result.status, Status::Working);
    assert_eq!(result.speed.unwrap().outcome, TransferOutcome::Cancelled);
    let fixture = Fixture::new(Mode::Stall);
    let start = Instant::now();
    let speed = fixture.control.speed.check(
        &fixture.proxy,
        Protocol::Http,
        &CheckSettings::default(),
        &fixture.control,
        start + Duration::from_millis(250),
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(speed.limit, TransferLimit::Unknown);
    assert!(speed.message.contains("timed out"));
}

#[test]
fn speed_samples_do_not_compete_with_each_other() {
    let fixture = Fixture::new(Mode::Complete);
    thread::scope(|scope| {
        for _ in 0..3 {
            scope.spawn(|| {
                assert_eq!(
                    fixture.check(true, 1).speed.unwrap().outcome,
                    TransferOutcome::Completed
                )
            });
        }
    });
    assert_eq!(fixture.stats.peak.load(Ordering::SeqCst), 1);
}

#[test]
fn waiting_for_the_speed_slot_is_cancellable() {
    let fixture = Fixture::new(Mode::Complete);
    let _guard = fixture.control.speed.gate.lock().unwrap();
    let start = Instant::now();
    thread::scope(|scope| {
        let worker = scope.spawn(|| {
            fixture.control.speed.check(
                &fixture.proxy,
                Protocol::Http,
                &CheckSettings::default(),
                &fixture.control,
                Instant::now() + Duration::from_secs(5),
            )
        });
        thread::sleep(Duration::from_millis(30));
        fixture.control.cancel();
        assert_eq!(worker.join().unwrap().outcome, TransferOutcome::Cancelled);
    });
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(fixture.stats.requests.lock().unwrap().is_empty());
}

#[test]
fn response_body_is_discarded_and_capped_even_without_content_length() {
    let mut download = Download::new(10);
    assert!(download.header(b"HTTP/1.1 200 OK\r\n"));
    assert!(download.header(b"Content-Type: application/octet-stream\r\n"));
    assert_eq!(download.write(&[0; 11]).unwrap(), 0);
    assert_eq!(download.bytes, 10);
    assert!(download.inspector.body.is_empty());
    assert!(download.invalid_payload);
}
