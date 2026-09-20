use proxy_vouch_core::{
    model::{CheckSettings, Status},
    parser::ImportOptions,
    session::{self, Session},
};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Stats {
    requests: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
    errors: Mutex<Vec<String>>,
}

impl Stats {
    fn error(&self, message: String) {
        self.errors
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(message);
    }
}

struct PendingResponse(Arc<Stats>);

impl PendingResponse {
    fn new(stats: &Arc<Stats>) -> Self {
        let active = stats.active.fetch_add(1, Ordering::SeqCst) + 1;
        stats.peak.fetch_max(active, Ordering::SeqCst);
        Self(Arc::clone(stats))
    }
}

impl Drop for PendingResponse {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn read_headers(
    reader: &mut impl Read,
    stop: &AtomicBool,
    deadline: Instant,
) -> io::Result<Option<Vec<u8>>> {
    let mut request = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Fixture request headers timed out",
            ));
        }
        match reader.read(&mut chunk) {
            // Cancellation may close a connection before a complete request is sent.
            Ok(0) => return Ok(None),
            Ok(read) => {
                request.extend_from_slice(&chunk[..read]);
                if request.len() > 32768 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Fixture request headers exceeded 32 KiB",
                    ));
                }
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    return Ok(Some(request));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                ) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error),
        }
    }
}

fn respond(
    mut stream: TcpStream,
    stop: &AtomicBool,
    stats: &Arc<Stats>,
    delay: Duration,
) -> io::Result<()> {
    // BSD/macOS accept can inherit O_NONBLOCK from the listener. Normalize the
    // stream before using blocking reads with a timeout on every platform.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(25)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let Some(request) = read_headers(&mut stream, stop, Instant::now() + Duration::from_secs(5))?
    else {
        return Ok(());
    };
    if !request.starts_with(b"GET http://check.invalid/") {
        let first_line = request
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Expected proxy GET request, received {:?}",
                String::from_utf8_lossy(&first_line[..first_line.len().min(160)])
            ),
        ));
    }
    stats.requests.fetch_add(1, Ordering::SeqCst);
    let pending = PendingResponse::new(stats);
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(5));
    }
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    // Release the fixture counter before the client can finish this response
    // and start another request. Thread cleanup can otherwise inflate the peak.
    drop(pending);
    let body = b"{\"ip\":\"198.51.100.7\"}";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.write_all(body)
}

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    stats: Arc<Stats>,
    thread: Option<thread::JoinHandle<()>>,
}

#[derive(Debug)]
struct Report {
    requests: usize,
    peak: usize,
    errors: Vec<String>,
}

impl Server {
    fn new(delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats::default());
        let thread_stop = Arc::clone(&stop);
        let thread_stats = Arc::clone(&stats);
        let handle = thread::spawn(move || {
            let mut handlers = Vec::new();
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let stop = Arc::clone(&thread_stop);
                        let stats = Arc::clone(&thread_stats);
                        handlers.push(thread::spawn(move || {
                            if let Err(error) = respond(stream, &stop, &stats, delay) {
                                stats.error(error.to_string());
                            }
                        }));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        thread_stats.error(error.to_string());
                        break;
                    }
                }
            }
            for handler in handlers {
                if handler.join().is_err() {
                    thread_stats.error("Fixture handler panicked".into());
                }
            }
        });
        Self {
            port,
            stop,
            stats,
            thread: Some(handle),
        }
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            if handle.join().is_err() {
                self.stats.error("Fixture accept thread panicked".into());
            }
        }
    }

    fn finish(mut self) -> Report {
        self.shutdown();
        Report {
            requests: self.stats.requests.load(Ordering::SeqCst),
            peak: self.stats.peak.load(Ordering::SeqCst),
            errors: self
                .stats
                .errors
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Cleanup must not panic while a test assertion is already unwinding.
        // Fixture failures are asserted explicitly through finish().
        self.shutdown();
    }
}

fn prepared(server: &Server, count: usize) -> session::SharedSession {
    let mut state = Session::default();
    state
        .preview(
            &vec![format!("http://127.0.0.1:{}", server.port); count].join("\n"),
            &ImportOptions::default(),
        )
        .unwrap();
    state.commit_import(false, true, false).unwrap();
    Arc::new(Mutex::new(state))
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        assert!(Instant::now() < deadline, "Condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_working(state: &session::SharedSession) {
    let state = state.lock().unwrap();
    let outcomes: Vec<_> = state
        .entries
        .iter()
        .map(|entry| {
            (
                entry.id,
                entry.status,
                entry
                    .result
                    .as_ref()
                    .map(|result| (&result.code, &result.message)),
            )
        })
        .collect();
    assert!(
        state
            .entries
            .iter()
            .all(|entry| entry.status == Status::Working),
        "Check outcomes: {outcomes:?}"
    );
}

#[test]
fn scheduler_obeys_concurrency_and_completes_all_requests() {
    let server = Server::new(Duration::from_millis(240));
    let state = prepared(&server, 6);
    let ids = state
        .lock()
        .unwrap()
        .entries
        .iter()
        .map(|entry| entry.id)
        .collect();
    let settings = CheckSettings {
        url: "http://check.invalid/".into(),
        country_lookup: false,
        anonymity_check: false,
        concurrency: 2,
        rate_limit: 100,
        ..CheckSettings::default()
    };
    session::start(Arc::clone(&state), ids, settings, false).unwrap();
    wait_until(Duration::from_secs(10), || !state.lock().unwrap().running);
    let report = server.finish();
    assert!(
        report.errors.is_empty(),
        "Fixture failures: {:?}",
        report.errors
    );
    assert_working(&state);
    assert_eq!(report.requests, 6);
    assert!(
        report.peak <= 2,
        "Exceeded configured concurrency: {report:?}"
    );
}

#[test]
fn scheduler_applies_the_global_rate_limit_across_workers() {
    let server = Server::new(Duration::ZERO);
    let state = prepared(&server, 6);
    let ids = state
        .lock()
        .unwrap()
        .entries
        .iter()
        .map(|entry| entry.id)
        .collect();
    let settings = CheckSettings {
        url: "http://check.invalid/".into(),
        country_lookup: false,
        anonymity_check: false,
        concurrency: 6,
        rate_limit: 10,
        ..CheckSettings::default()
    };
    let started = Instant::now();
    session::start(Arc::clone(&state), ids, settings, false).unwrap();
    wait_until(Duration::from_secs(10), || !state.lock().unwrap().running);
    let elapsed = started.elapsed();
    let report = server.finish();
    assert!(
        report.errors.is_empty(),
        "Fixture failures: {:?}",
        report.errors
    );
    assert_working(&state);
    assert_eq!(report.requests, 6);
    // Six admissions at 10/s need at least five 100 ms intervals. OS/network
    // delays can only increase this lower bound; handler arrival gaps can shrink.
    assert!(
        elapsed >= Duration::from_millis(500),
        "Global rate limit was bypassed: {elapsed:?}"
    );
}

#[test]
fn cancellation_releases_workers_and_prevents_mutation_during_a_run() {
    let server = Server::new(Duration::from_secs(8));
    let state = prepared(&server, 8);
    let ids = state
        .lock()
        .unwrap()
        .entries
        .iter()
        .map(|entry| entry.id)
        .collect();
    let settings = CheckSettings {
        url: "http://check.invalid/".into(),
        country_lookup: false,
        anonymity_check: false,
        concurrency: 2,
        rate_limit: 100,
        ..CheckSettings::default()
    };
    session::start(Arc::clone(&state), ids, settings, false).unwrap();
    wait_until(Duration::from_secs(5), || {
        server.stats.active.load(Ordering::SeqCst) == 2
    });
    assert!(state.lock().unwrap().clear(&[]).is_err());
    let start = Instant::now();
    state.lock().unwrap().control.as_ref().unwrap().cancel();
    wait_until(Duration::from_secs(1), || !state.lock().unwrap().running);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(state
        .lock()
        .unwrap()
        .entries
        .iter()
        .all(|entry| entry.status == Status::Cancelled));
    state.lock().unwrap().clear(&[]).unwrap();
    assert!(state.lock().unwrap().entries.is_empty());
    let report = server.finish();
    assert!(
        report.errors.is_empty(),
        "Fixture failures: {:?}",
        report.errors
    );
}

struct Fragments<'a> {
    remaining: &'a [u8],
    max_read: usize,
    errors: VecDeque<io::ErrorKind>,
}

impl Read for Fragments<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if let Some(error) = self.errors.pop_front() {
            return Err(error.into());
        }
        let length = self.remaining.len().min(self.max_read).min(output.len());
        output[..length].copy_from_slice(&self.remaining[..length]);
        self.remaining = &self.remaining[length..];
        Ok(length)
    }
}

#[test]
fn fixture_reads_fragmented_headers_after_transient_io_errors() {
    let request = b"GET http://check.invalid/ HTTP/1.1\r\nHost: check.invalid\r\n\r\n";
    for max_read in [1, 2, 7, request.len()] {
        let mut reader = Fragments {
            remaining: request,
            max_read,
            errors: [
                io::ErrorKind::Interrupted,
                io::ErrorKind::WouldBlock,
                io::ErrorKind::TimedOut,
            ]
            .into(),
        };
        let headers = read_headers(
            &mut reader,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(headers.as_deref(), Some(request.as_slice()));
    }
}

#[test]
fn fixture_accepts_a_disconnect_before_headers_are_complete() {
    let mut partial = &b"GET http://check.inva"[..];
    assert!(read_headers(
        &mut partial,
        &AtomicBool::new(false),
        Instant::now() + Duration::from_secs(1)
    )
    .unwrap()
    .is_none());
}

#[test]
fn fixture_handles_an_initially_nonblocking_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream.set_nonblocking(true).unwrap();
    let stats = Arc::new(Stats::default());
    let worker_stats = Arc::clone(&stats);
    let worker = thread::spawn(move || {
        respond(
            stream,
            &AtomicBool::new(false),
            &worker_stats,
            Duration::ZERO,
        )
    });
    client.write_all(b"G").unwrap();
    thread::sleep(Duration::from_millis(10));
    client
        .write_all(b"ET http://check.invalid/ HTTP/1.1\r\nHost: check.invalid\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    worker.join().unwrap().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("{\"ip\":\"198.51.100.7\"}"));
    assert_eq!(stats.requests.load(Ordering::SeqCst), 1);
    assert_eq!(stats.active.load(Ordering::SeqCst), 0);
}

#[test]
fn invalid_requests_are_reported_without_panicking_in_worker_cleanup() {
    let server = Server::new(Duration::ZERO);
    let mut client = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    client.write_all(b"INVALID\r\n\r\n").unwrap();
    wait_until(Duration::from_secs(5), || {
        !server.stats.errors.lock().unwrap().is_empty()
    });
    let report = server.finish();
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].contains("Expected proxy GET request"));
}

#[test]
fn fixture_cleanup_preserves_the_original_panic() {
    let stats = Arc::new(Stats::default());
    let result = std::panic::catch_unwind(|| {
        let _server = Server {
            port: 0,
            stop: Arc::new(AtomicBool::new(false)),
            stats: Arc::clone(&stats),
            thread: Some(thread::spawn(|| panic!("synthetic fixture failure"))),
        };
        panic!("original test failure");
    });
    assert_eq!(
        result.unwrap_err().downcast_ref::<&str>(),
        Some(&"original test failure")
    );
    assert_eq!(
        stats.errors.lock().unwrap().as_slice(),
        &["Fixture accept thread panicked"]
    );
}
