use crate::model::{safe_url, Attempt, CheckResult, CheckSettings, Protocol, Proxy, Status};
use curl::{
    easy::{Auth, Easy2, Handler, InfoType, WriteError},
    multi::Multi,
};
use socket2::Socket;
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// Per-run cancellation, request pacing and endpoint health, shared by all workers.
pub struct Control {
    pub cancelled: AtomicBool,
    next_request: Mutex<Instant>,
    endpoints: Mutex<HashMap<String, VecDeque<bool>>>,
    /// Explicit trust anchor used by controlled fixtures; never exposed as a TLS bypass.
    pub ca_file: Option<PathBuf>,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            next_request: Mutex::new(Instant::now()),
            endpoints: Mutex::new(HashMap::new()),
            ca_file: None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Permit {
    Granted,
    WaitUntil(Instant),
    Stopped,
}

impl Control {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    fn wait_until(&self, until: Instant, deadline: Instant) -> bool {
        while Instant::now() < until {
            if self.is_cancelled() || Instant::now() >= deadline {
                return false;
            }
            thread::sleep(
                Duration::from_millis(10).min(until.saturating_duration_since(Instant::now())),
            );
        }
        !self.is_cancelled() && Instant::now() < deadline
    }
    fn request_permit(
        &self,
        rate: u32,
        deadline: Instant,
        clock: impl FnOnce() -> Instant,
    ) -> Permit {
        let Ok(mut next) = self.next_request.lock() else {
            return Permit::Stopped;
        };
        // Sample the clock under the same lock as the admission decision. The
        // injected clock lets tests verify boundaries without OS scheduling jitter.
        let now = clock();
        if self.is_cancelled() || now >= deadline {
            return Permit::Stopped;
        }
        if now < *next {
            return Permit::WaitUntil(*next);
        }
        *next = now + Duration::from_secs_f64(1.0 / rate as f64);
        Permit::Granted
    }

    fn acquire(&self, rate: u32, deadline: Instant) -> bool {
        loop {
            match self.request_permit(rate, deadline, Instant::now) {
                Permit::Granted => return true,
                Permit::Stopped => return false,
                Permit::WaitUntil(until) => {
                    if !self.wait_until(until, deadline) {
                        return false;
                    }
                }
            }
        }
    }
    fn endpoint_paused(&self, url: &str) -> bool {
        self.endpoints
            .lock()
            .ok()
            .and_then(|states| states.get(url).cloned())
            .is_some_and(|states| {
                let failures = states.iter().filter(|v| **v).count();
                failures >= 5 && failures * 100 >= states.len() * 80
            })
    }
    fn record_response(&self, url: &str, code: u32) {
        if code == 0 {
            return;
        }
        if let Ok(mut endpoints) = self.endpoints.lock() {
            let window = endpoints.entry(url.to_owned()).or_default();
            window.push_back(code == 429 || code >= 500);
            if window.len() > 20 {
                window.pop_front();
            }
        }
    }
}

#[derive(Default)]
struct Response {
    sockets: Vec<Socket>,
    tcp_connected: bool,
    body: Vec<u8>,
    header_bytes: usize,
    too_large: bool,
    auth_failed: bool,
    auth_unsupported: bool,
    socks_granted: bool,
    proxy_challenged: bool,
    auth_sent: bool,
    local_dns_failed: bool,
    socks_rejected: bool,
    basic_offered: bool,
    other_auth_offered: bool,
    target_request_sent: bool,
    address_unsupported: bool,
}

impl Response {
    fn observe_tcp_connection(&mut self) {
        // Older libcurl reports connect_time/primary_ip only after SOCKS succeeds.
        // Query the OS while the handshake is pending, before curl closes on error.
        self.tcp_connected |= self.sockets.iter().any(|socket| socket.peer_addr().is_ok());
        if self.tcp_connected {
            self.sockets.clear();
        }
    }
}

impl Handler for Response {
    fn open_socket(
        &mut self,
        family: std::ffi::c_int,
        socktype: std::ffi::c_int,
        protocol: std::ffi::c_int,
    ) -> Option<curl_sys::curl_socket_t> {
        // Match curl's default CLOEXEC socket creation. Owned duplicates avoid
        // inspecting a closed/reused descriptor; they never read or write data.
        // Release them on connection, or when this attempt ends (including cancel).
        let socket = Socket::new(family.into(), socktype.into(), Some(protocol.into())).ok()?;
        if !self.tcp_connected {
            self.sockets.push(socket.try_clone().ok()?);
        }
        #[cfg(unix)]
        {
            use std::os::fd::IntoRawFd;
            Some(socket.into_raw_fd())
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::IntoRawSocket;
            Some(socket.into_raw_socket())
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        if self.body.len() + data.len() > 65536 {
            self.too_large = true;
            return Ok(0);
        }
        self.body.extend_from_slice(data);
        Ok(data.len())
    }
    fn header(&mut self, data: &[u8]) -> bool {
        self.header_bytes += data.len();
        if self.header_bytes > 32768 {
            self.too_large = true;
            return false;
        }
        if data.starts_with(b"HTTP/") {
            self.body.clear();
            self.proxy_challenged |=
                String::from_utf8_lossy(data).split_whitespace().nth(1) == Some("407");
        }
        let lower = String::from_utf8_lossy(data).to_ascii_lowercase();
        if lower.starts_with("proxy-authenticate:") {
            self.basic_offered |= lower.contains("basic");
            self.other_auth_offered |= !lower.contains("basic");
        }
        true
    }
    fn debug(&mut self, kind: InfoType, data: &[u8]) {
        // Never retain or forward libcurl debug output: it can include credentials.
        if matches!(kind, InfoType::Text) {
            let text = String::from_utf8_lossy(data).to_ascii_lowercase();
            self.auth_failed |=
                text.contains("user was rejected") || text.contains("authentication failed");
            self.auth_unsupported |= text.contains("no authentication method")
                || text.contains("gssapi")
                || text.contains("unknown socks5 mode");
            self.socks_granted |= text.contains("request granted");
            self.local_dns_failed |= text.contains("could not resolve")
                || text.contains("cannot resolve")
                || text.contains("failed to resolve");
            self.socks_rejected |= text.contains("cannot complete socks5 connection")
                || text.contains("can't complete socks5 connection")
                || text.contains("request rejected");
            self.address_unsupported |=
                text.contains("socks5 connection") && text.contains(". (8)");
        } else if matches!(kind, InfoType::HeaderOut) {
            self.target_request_sent |= data.starts_with(b"GET ");
            self.auth_sent |= String::from_utf8_lossy(data)
                .to_ascii_lowercase()
                .contains("proxy-authorization:");
        }
    }
}

fn attempt(
    protocol: Protocol,
    url: &str,
    status: Status,
    code: &str,
    stage: &str,
    message: &str,
) -> Attempt {
    Attempt {
        protocol,
        detected: None,
        status,
        authentication: "Not tested".into(),
        code: code.into(),
        stage: stage.into(),
        message: message.into(),
        duration_ms: 0,
        exit_ip: None,
        country_code: None,
        check_url: safe_url(url),
    }
}

fn cancelled_attempt(protocol: Protocol, url: &str, control: &Control) -> Attempt {
    if control.is_cancelled() {
        attempt(
            protocol,
            url,
            Status::Cancelled,
            "CANCELLED",
            "queue",
            "Check cancelled.",
        )
    } else {
        attempt(
            protocol,
            url,
            Status::Inconclusive,
            "DETECTION_INCOMPLETE",
            "deadline",
            "The total check deadline was reached.",
        )
    }
}

fn restrict_socks_auth(easy: &mut Easy2<Response>) -> Result<(), curl::Error> {
    // curl's safe wrapper does not expose CURLOPT_SOCKS5_AUTH. Its stable value
    // is CURLOPTTYPE_LONG + 267 in both the system and bundled curl headers.
    const SOCKS5_AUTH: curl_sys::CURLoption = curl_sys::CURLOPTTYPE_LONG + 267;
    // SAFETY: easy owns this live handle exclusively. This option accepts a C long,
    // and CURLAUTH_BASIC allows username/password while excluding ambient GSSAPI.
    let code = unsafe {
        curl_sys::curl_easy_setopt(
            easy.raw(),
            SOCKS5_AUTH,
            curl_sys::CURLAUTH_BASIC as std::ffi::c_long,
        )
    };
    if code == curl_sys::CURLE_OK {
        Ok(())
    } else {
        Err(curl::Error::new(code))
    }
}

fn probe(
    proxy: &Proxy,
    protocol: Protocol,
    url: &str,
    settings: &CheckSettings,
    control: &Control,
    deadline: Instant,
) -> Attempt {
    if control.endpoint_paused(url) {
        return attempt(
            protocol,
            url,
            Status::Inconclusive,
            "ENDPOINT_UNAVAILABLE",
            "target",
            "Check endpoint unavailable. Change the profile or start a new run.",
        );
    }
    if !control.acquire(settings.rate_limit, deadline) {
        return cancelled_attempt(protocol, url, control);
    }
    let start = Instant::now();
    let mut easy = Easy2::new(Response::default());
    let setup = (|| -> Result<(), curl::Error> {
        easy.url(url)?;
        easy.proxy(&format!("{}://{}", protocol.scheme(), proxy.address()))?;
        easy.noproxy("")?;
        easy.follow_location(false)?;
        easy.connect_timeout(
            Duration::from_millis(settings.connect_timeout_ms)
                .min(deadline.saturating_duration_since(start)),
        )?;
        easy.timeout(
            Duration::from_millis(settings.attempt_timeout_ms)
                .min(deadline.saturating_duration_since(start)),
        )?;
        easy.ssl_verify_peer(true)?;
        easy.ssl_verify_host(true)?;
        easy.proxy_ssl_verify_peer(true)?;
        easy.proxy_ssl_verify_host(true)?;
        easy.useragent(concat!("ProxyVouch/", env!("CARGO_PKG_VERSION")))?;
        easy.accept_encoding("")?;
        easy.verbose(true)?;
        if matches!(protocol, Protocol::Socks5 | Protocol::Socks5h) {
            restrict_socks_auth(&mut easy)?;
        }
        if let Some(ca) = &control.ca_file {
            easy.cainfo(ca)?;
            easy.proxy_cainfo(&ca.to_string_lossy())?;
        }
        if let Some(credentials) = &proxy.credentials {
            easy.proxy_username(&credentials.username)?;
            if let Some(password) = &credentials.password {
                easy.proxy_password(password)?;
            }
            easy.proxy_auth(Auth::new().basic(true))?;
        }
        Ok(())
    })();
    if setup.is_err() {
        return attempt(
            protocol,
            url,
            Status::Inconclusive,
            "CLIENT_CONFIGURATION_ERROR",
            "client",
            "The network library could not configure this proxy mode.",
        );
    }
    let multi = Multi::new();
    let Ok(mut handle) = multi.add2(easy) else {
        return attempt(
            protocol,
            url,
            Status::Inconclusive,
            "INTERNAL_ERROR",
            "client",
            "Could not create the request.",
        );
    };
    let completion = loop {
        if control.is_cancelled() || Instant::now() >= deadline {
            return cancelled_attempt(protocol, url, control);
        }
        if multi.perform().is_err() {
            return attempt(
                protocol,
                url,
                Status::Inconclusive,
                "INTERNAL_ERROR",
                "client",
                "The network event loop failed.",
            );
        }
        handle.get_mut().observe_tcp_connection();
        let mut result = None;
        multi.messages(|message| {
            if let Some(value) = message.result_for2(&handle) {
                result = Some(value);
            }
        });
        if let Some(result) = result {
            break result;
        }
        if multi.wait(&mut [], Duration::from_millis(25)).unwrap_or(0) == 0 {
            thread::sleep(Duration::from_millis(2));
        }
    };
    let response_code = handle.response_code().unwrap_or(0);
    let connect_code = handle.http_connectcode().unwrap_or(0);
    let tcp_connected = handle.get_ref().tcp_connected
        || handle.connect_time().unwrap_or_default() > Duration::ZERO
        || handle
            .primary_ip()
            .ok()
            .flatten()
            .is_some_and(|ip| !ip.is_empty());
    let data = handle.get_ref();
    let tunnel = (200..300).contains(&connect_code) || data.socks_granted;
    let detected = if tunnel
        || connect_code == 407
        || response_code == 407
        || data.auth_failed
        || data.socks_rejected
    {
        Some(protocol)
    } else {
        None
    };
    let mut result = if data.too_large {
        attempt(
            protocol,
            url,
            Status::Inconclusive,
            "RESPONSE_TOO_LARGE",
            "target",
            "Response exceeded the header or body limit.",
        )
    } else if data.other_auth_offered && !data.basic_offered {
        attempt(
            protocol,
            url,
            Status::Inconclusive,
            "AUTH_METHOD_UNSUPPORTED",
            "authentication",
            "The proxy requires an authentication method other than Basic.",
        )
    } else if response_code == 407 || connect_code == 407 || data.auth_failed {
        let missing = proxy.credentials.is_none();
        attempt(
            protocol,
            url,
            Status::Failed,
            if missing {
                "AUTH_REQUIRED"
            } else {
                "AUTH_FAILED"
            },
            "authentication",
            if missing {
                "The proxy requires credentials."
            } else {
                "The proxy rejected the supplied credentials."
            },
        )
    } else if data.auth_unsupported {
        attempt(
            protocol,
            url,
            Status::Inconclusive,
            "AUTH_METHOD_UNSUPPORTED",
            "authentication",
            "The proxy did not accept a supported authentication method.",
        )
    } else if data.local_dns_failed
        && !completion
            .as_ref()
            .is_err_and(|error| error.is_couldnt_resolve_proxy())
    {
        attempt(
            protocol,
            url,
            Status::Inconclusive,
            "LOCAL_DNS_FAILED",
            "target",
            "The destination hostname could not be resolved locally.",
        )
    } else if data.address_unsupported {
        attempt(
            protocol,
            url,
            Status::Inconclusive,
            "ADDRESS_TYPE_UNSUPPORTED",
            "proxy",
            "The proxy does not support this destination address type.",
        )
    } else if data.socks_rejected {
        attempt(
            protocol,
            url,
            Status::Failed,
            "SOCKS_REQUEST_REJECTED",
            "proxy",
            "The SOCKS proxy rejected the destination request.",
        )
    } else if connect_code >= 300 {
        attempt(
            protocol,
            url,
            Status::Failed,
            "CONNECT_DENIED",
            "proxy",
            "The proxy rejected the CONNECT request for this destination.",
        )
    } else if let Err(err) = completion {
        if err.is_peer_failed_verification() || err.is_ssl_certproblem() {
            let proxy_tls = protocol == Protocol::Https && !tunnel;
            attempt(
                protocol,
                url,
                if proxy_tls {
                    Status::Failed
                } else {
                    Status::Inconclusive
                },
                if proxy_tls {
                    "PROXY_TLS_INVALID"
                } else {
                    "TARGET_TLS_INVALID"
                },
                if proxy_tls { "proxy_tls" } else { "target" },
                "Certificate chain or hostname verification failed.",
            )
        } else if err.is_couldnt_resolve_proxy() {
            attempt(
                protocol,
                url,
                Status::Failed,
                "PROXY_DNS_FAILED",
                "proxy_dns",
                "The proxy hostname could not be resolved.",
            )
        } else if err.is_couldnt_resolve_host() {
            attempt(
                protocol,
                url,
                Status::Inconclusive,
                "LOCAL_DNS_FAILED",
                "target",
                "The destination hostname could not be resolved locally.",
            )
        } else if err.is_couldnt_connect() && !tcp_connected {
            attempt(
                protocol,
                url,
                Status::Failed,
                "CONNECTION_REFUSED",
                "connect",
                "Could not connect to the proxy address.",
            )
        } else if err.is_operation_timedout() {
            attempt(
                protocol,
                url,
                if tcp_connected {
                    Status::Inconclusive
                } else {
                    Status::Failed
                },
                if data.target_request_sent || tunnel {
                    "TARGET_TIMEOUT"
                } else if tcp_connected {
                    "PROXY_HANDSHAKE_TIMEOUT"
                } else {
                    "CONNECT_TIMEOUT"
                },
                if data.target_request_sent || tunnel {
                    "target"
                } else if tcp_connected {
                    "protocol"
                } else {
                    "connect"
                },
                "The network operation timed out.",
            )
        } else if tunnel {
            attempt(
                protocol,
                url,
                Status::Inconclusive,
                "TARGET_HTTP_ERROR",
                "target",
                "The destination request failed after the proxy tunnel was established.",
            )
        } else {
            attempt(
                protocol,
                url,
                Status::Inconclusive,
                "PROTOCOL_NOT_DETECTED",
                "protocol",
                "No usable proxy response was received for this protocol.",
            )
        }
    } else {
        control.record_response(url, response_code);
        if response_code != settings.expected_status as u32 {
            attempt(
                protocol,
                url,
                Status::Inconclusive,
                "TARGET_HTTP_ERROR",
                "target",
                &format!(
                    "The check endpoint returned HTTP {response_code}; expected {}.",
                    settings.expected_status
                ),
            )
        } else if !settings.body_contains.is_empty()
            && !String::from_utf8_lossy(&data.body).contains(&settings.body_contains)
        {
            attempt(
                protocol,
                url,
                Status::Inconclusive,
                "UNEXPECTED_RESPONSE",
                "target",
                "The response did not contain the expected text.",
            )
        } else {
            let json = serde_json::from_slice::<serde_json::Value>(&data.body).ok();
            let ip = json.as_ref().and_then(|v| {
                v.get("ip")
                    .and_then(|ip| ip.as_str())
                    .and_then(|ip| ip.parse::<IpAddr>().ok())
                    .map(|ip| ip.to_string())
            });
            if settings.ip_echo && ip.is_none() {
                attempt(
                    protocol,
                    url,
                    Status::Inconclusive,
                    "UNEXPECTED_RESPONSE",
                    "target",
                    "Expected a JSON object with a valid IP address in the ip field.",
                )
            } else {
                let mut result = attempt(
                    protocol,
                    url,
                    Status::Working,
                    "",
                    "complete",
                    "The check request completed successfully.",
                );
                result.detected = Some(protocol);
                if settings.ip_echo {
                    result.exit_ip = ip;
                }
                result.country_code = json.as_ref().and_then(|value| {
                    value
                        .get("country")?
                        .as_str()
                        .filter(|code| {
                            code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_alphabetic())
                        })
                        .map(str::to_ascii_uppercase)
                });
                result
            }
        }
    };
    result.detected = result.detected.or(detected);
    result.authentication = if result.code == "AUTH_FAILED" || result.code == "AUTH_REQUIRED" {
        "Failed"
    } else if data.proxy_challenged && data.auth_sent && result.status == Status::Working {
        "Authenticated"
    } else if proxy.credentials.is_none() && result.status == Status::Working {
        "Not required"
    } else {
        "Not tested"
    }
    .into();
    result.duration_ms = start.elapsed().as_millis() as u64;
    result
}

pub fn check(
    proxy: &Proxy,
    settings: &CheckSettings,
    control: &Arc<Control>,
    preferred: Option<Protocol>,
) -> CheckResult {
    check_with_country_url(
        proxy,
        settings,
        control,
        preferred,
        "https://api.country.is/",
    )
}

fn check_with_country_url(
    proxy: &Proxy,
    settings: &CheckSettings,
    control: &Arc<Control>,
    preferred: Option<Protocol>,
    country_url: &str,
) -> CheckResult {
    let started = Instant::now();
    let deadline = started + Duration::from_millis(settings.total_timeout_ms);
    let mut protocols = if proxy.protocol == Protocol::Auto {
        vec![
            Protocol::Https,
            Protocol::Socks5h,
            Protocol::Http,
            Protocol::Socks4a,
            Protocol::Socks4,
        ]
    } else {
        vec![proxy.protocol]
    };
    if proxy.protocol == Protocol::Auto {
        if let Some(preferred) = preferred {
            if let Some(i) = protocols.iter().position(|p| *p == preferred) {
                protocols.remove(i);
                protocols.insert(0, preferred);
            }
        }
    }
    let mut attempts = Vec::new();
    'candidates: for protocol in protocols {
        if proxy.validate_auth(protocol).is_err() {
            attempts.push(attempt(
                protocol,
                &settings.url,
                Status::Inconclusive,
                "UNSUPPORTED_AUTH",
                "client",
                "This candidate cannot represent the supplied credentials and was skipped.",
            ));
            continue;
        }
        for url in [&settings.url, &settings.fallback_url]
            .into_iter()
            .filter(|u| !u.is_empty())
        {
            for retry in 0..=settings.retries {
                let result = probe(proxy, protocol, url, settings, control, deadline);
                let success = result.status == Status::Working;
                let cancelled =
                    result.status == Status::Cancelled || result.code == "DETECTION_INCOMPLETE";
                let temporary = matches!(
                    result.code.as_str(),
                    "CONNECT_TIMEOUT" | "TARGET_TIMEOUT" | "CONNECTION_RESET"
                );
                let target_error = result.stage == "target";
                attempts.push(result);
                if success || cancelled {
                    break 'candidates;
                }
                if temporary && retry < settings.retries {
                    if !control.wait_until(
                        Instant::now() + Duration::from_millis(500 * (retry as u64 + 1)),
                        deadline,
                    ) {
                        attempts.push(cancelled_attempt(protocol, url, control));
                        break 'candidates;
                    }
                } else if !target_error {
                    continue 'candidates;
                } else {
                    break;
                }
            }
        }
    }
    let selected = attempts
        .iter()
        .rev()
        .find(|a| {
            a.status == Status::Working
                || a.status == Status::Cancelled
                || a.code == "DETECTION_INCOMPLETE"
        })
        .or_else(|| {
            attempts.iter().find(|a| {
                a.status == Status::Failed
                    && (a.detected.is_some() || a.code == "PROXY_TLS_INVALID")
            })
        })
        .or_else(|| attempts.iter().find(|a| a.stage == "target"))
        .or_else(|| attempts.iter().find(|a| a.status == Status::Failed))
        .or_else(|| attempts.first());
    let mut result = CheckResult {
        status: Status::Inconclusive,
        detected: None,
        authentication: "Not tested".into(),
        latency_ms: None,
        total_duration_ms: started.elapsed().as_millis() as u64,
        exit_ip: None,
        country_code: None,
        checked_at: chrono::Utc::now().to_rfc3339(),
        code: "PROTOCOL_NOT_DETECTED".into(),
        stage: "protocol".into(),
        message: "No supported protocol could complete this check.".into(),
        check_url: settings.safe_url(),
        attempts: Vec::new(),
        settings: settings.clone(),
    };
    if let Some(selected) = selected {
        result.status = selected.status;
        result.detected = selected.detected;
        result.authentication = selected.authentication.clone();
        result.code = selected.code.clone();
        result.stage = selected.stage.clone();
        result.message = selected.message.clone();
        result.check_url = selected.check_url.clone();
        result.exit_ip = selected.exit_ip.clone();
        if selected.status == Status::Working {
            result.latency_ms = Some(selected.duration_ms);
        }
    }
    result.attempts = attempts;
    if settings.country_lookup && result.status == Status::Working {
        if let Some(protocol) = result.detected {
            // Query the measured exit IP when available: rotating proxies can use
            // another exit for the next request. Custom checks use the caller IP.
            let url = format!(
                "{country_url}{}",
                result.exit_ip.as_deref().unwrap_or_default()
            );
            let country_settings = CheckSettings {
                ip_echo: true,
                expected_status: 200,
                body_contains: String::new(),
                rate_limit: settings.rate_limit.min(10),
                ..settings.clone()
            };
            let country = probe(
                proxy,
                protocol,
                &url,
                &country_settings,
                control,
                deadline.min(Instant::now() + Duration::from_secs(5)),
            );
            // Optional metadata must never overwrite the availability verdict,
            // its latency, or the attempts that established it.
            if country.status == Status::Working
                && (result.exit_ip.is_none() || country.exit_ip == result.exit_ip)
            {
                result.country_code = country.country_code;
                if result.country_code.is_some() && result.exit_ip.is_none() {
                    result.exit_ip = country.exit_ip;
                }
            }
        }
    }
    result.total_duration_ms = started.elapsed().as_millis() as u64;
    result
}

#[cfg(test)]
mod country_tests {
    use super::*;
    use crate::parser::parse_line;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn fixture(
        settings: CheckSettings,
        country_response: &str,
        cancel_country: bool,
    ) -> (CheckResult, Vec<String>) {
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
        let response = country_response.to_owned();
        let control = Arc::new(Control::default());
        let server_control = Arc::clone(&control);
        let finished = Arc::new(AtomicBool::new(false));
        let server_finished = Arc::clone(&finished);
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            while !server_finished.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("Fixture accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 32768);
                }
                let request = String::from_utf8(request).unwrap();
                let country = request.starts_with("GET http://country.invalid/");
                let custom = request.starts_with("GET http://check.invalid/custom");
                requests.push(request);
                if country && cancel_country {
                    server_control.cancel();
                    continue;
                }
                let (status, body) = if country {
                    (200, response.as_str())
                } else if custom {
                    (201, "accepted")
                } else {
                    (200, r#"{"ip":"198.51.100.9"}"#)
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        let result =
            check_with_country_url(&proxy, &settings, &control, None, "http://country.invalid/");
        finished.store(true, Ordering::SeqCst);
        (result, server.join().unwrap())
    }

    fn settings() -> CheckSettings {
        CheckSettings {
            url: "http://check.invalid/".into(),
            rate_limit: 100,
            ..CheckSettings::default()
        }
    }

    #[test]
    fn country_lookup_uses_the_authenticated_proxy_and_measured_exit_ip() {
        let (result, requests) =
            fixture(settings(), r#"{"ip":"198.51.100.9","country":"de"}"#, false);
        assert_eq!(result.status, Status::Working);
        assert_eq!(result.country_code.as_deref(), Some("DE"));
        assert_eq!(result.attempts.len(), 1);
        assert_eq!(result.latency_ms, Some(result.attempts[0].duration_ms));
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("GET http://country.invalid/198.51.100.9 HTTP/1.1\r\n"));
        assert!(requests[1].contains("Proxy-Authorization: Basic "));
    }

    #[test]
    fn disabling_country_lookup_makes_no_extra_request() {
        let (result, requests) = fixture(
            CheckSettings {
                country_lookup: false,
                ..settings()
            },
            "",
            false,
        );
        assert_eq!(result.status, Status::Working);
        assert!(result.country_code.is_none());
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn country_lookup_has_independent_response_rules_for_custom_checks() {
        let (result, requests) = fixture(
            CheckSettings {
                url: "http://check.invalid/custom".into(),
                ip_echo: false,
                expected_status: 201,
                body_contains: "accepted".into(),
                ..settings()
            },
            r#"{"ip":"198.51.100.9","country":"US"}"#,
            false,
        );
        assert_eq!(result.status, Status::Working);
        assert_eq!(result.country_code.as_deref(), Some("US"));
        assert_eq!(result.exit_ip.as_deref(), Some("198.51.100.9"));
        assert!(requests[1].starts_with("GET http://country.invalid/ HTTP/1.1\r\n"));
    }

    #[test]
    fn unavailable_or_invalid_country_data_does_not_fail_a_working_proxy() {
        for body in [
            "unavailable",
            "{}",
            r#"{"ip":"198.51.100.9","country":null}"#,
            r#"{"ip":"198.51.100.9","country":"USA"}"#,
            r#"{"ip":"198.51.100.9","country":"12"}"#,
            r#"{"ip":"198.51.100.10","country":"DE"}"#,
        ] {
            let (result, _) = fixture(settings(), body, false);
            assert_eq!(result.status, Status::Working);
            assert!(result.country_code.is_none());
            assert_eq!(result.exit_ip.as_deref(), Some("198.51.100.9"));
        }
    }

    #[test]
    fn cancellation_during_country_lookup_keeps_the_completed_check() {
        let started = Instant::now();
        let (result, requests) = fixture(settings(), "", true);
        assert_eq!(result.status, Status::Working);
        assert!(result.country_code.is_none());
        assert_eq!(requests.len(), 2);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

#[cfg(test)]
mod pacing_tests {
    use super::*;
    use std::sync::Barrier;

    fn control_at(now: Instant) -> Control {
        Control {
            next_request: Mutex::new(now),
            ..Control::default()
        }
    }

    #[test]
    fn admits_only_at_the_configured_interval_boundary() {
        for (rate, interval) in [
            (1, Duration::from_secs(1)),
            (10, Duration::from_millis(100)),
            (100, Duration::from_millis(10)),
        ] {
            let now = Instant::now();
            let control = control_at(now);
            let deadline = now + Duration::from_secs(30);
            assert_eq!(
                control.request_permit(rate, deadline, || now),
                Permit::Granted
            );
            assert_eq!(
                control.request_permit(rate, deadline, || now),
                Permit::WaitUntil(now + interval)
            );
            assert_eq!(
                control.request_permit(rate, deadline, || now + interval - Duration::from_nanos(1)),
                Permit::WaitUntil(now + interval)
            );
            assert_eq!(
                control.request_permit(rate, deadline, || now + interval),
                Permit::Granted
            );
        }
    }

    #[test]
    fn a_delayed_worker_cannot_accumulate_a_burst_of_permits() {
        let now = Instant::now();
        let control = control_at(now);
        let delayed = now + Duration::from_secs(10);
        let deadline = delayed + Duration::from_secs(1);
        assert_eq!(
            control.request_permit(10, deadline, || now),
            Permit::Granted
        );
        assert_eq!(
            control.request_permit(10, deadline, || delayed),
            Permit::Granted
        );
        for _ in 0..10 {
            assert_eq!(
                control.request_permit(10, deadline, || delayed),
                Permit::WaitUntil(delayed + Duration::from_millis(100))
            );
        }
    }

    #[test]
    fn simultaneous_workers_share_one_global_permit() {
        let now = Instant::now();
        let control = control_at(now);
        let barrier = Barrier::new(16);
        let granted = thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        control.request_permit(10, now + Duration::from_secs(1), || now)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|permit| *permit == Permit::Granted)
                .count()
        });
        assert_eq!(granted, 1);
    }

    #[test]
    fn cancelled_or_expired_checks_do_not_consume_a_permit() {
        let now = Instant::now();
        let control = control_at(now);
        assert_eq!(control.request_permit(10, now, || now), Permit::Stopped);
        assert_eq!(*control.next_request.lock().unwrap(), now);
        control.cancel();
        assert_eq!(
            control.request_permit(10, now + Duration::from_secs(1), || now),
            Permit::Stopped
        );
        assert_eq!(*control.next_request.lock().unwrap(), now);
        assert!(!control.acquire(10, now + Duration::from_secs(1)));
    }
}
