use super::{
    configure_request, CheckSettings, Control, Easy2, Handler, InfoType, Multi, Protocol, Proxy,
    Response, WriteError,
};
use crate::model::{safe_url, SpeedResult, TransferLimit, TransferOutcome};
use curl::easy::List;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, TryLockError,
    },
    thread,
    time::{Duration, Instant},
};

const DOWNLOAD_URL: &str = "https://speed.cloudflare.com/__down";
static REQUEST_ID: AtomicU64 = AtomicU64::new(0);

pub(super) struct Check {
    url: String,
    gate: Mutex<()>,
}

impl Default for Check {
    fn default() -> Self {
        Self::new(DOWNLOAD_URL)
    }
}

impl Check {
    fn new(url: &str) -> Self {
        Self {
            url: url.into(),
            gate: Mutex::new(()),
        }
    }

    pub(super) fn check(
        &self,
        proxy: &Proxy,
        protocol: Protocol,
        settings: &CheckSettings,
        control: &Control,
        deadline: Instant,
    ) -> SpeedResult {
        let mut result = SpeedResult {
            outcome: TransferOutcome::Skipped,
            download_mbps: None,
            requested_bytes: u64::from(settings.speed_test_mib) * 1024 * 1024,
            received_bytes: 0,
            duration_ms: 0,
            limit: TransferLimit::Unknown,
            http_status: None,
            proxy_http_status: None,
            check_url: safe_url(&self.url),
            message: String::new(),
        };
        if !(1..=32).contains(&settings.speed_test_mib) {
            result.message = "Choose a speed sample between 1 and 32 MiB.".into();
            return result;
        }
        // Large downloads must not compete with other speed samples in this run.
        // Waiting remains cancellable and uses the proxy's remaining total budget.
        let _guard = loop {
            if control.is_cancelled() {
                result.outcome = TransferOutcome::Cancelled;
                result.message =
                    "Speed test cancelled before downloading. Traffic quota is unknown.".into();
                return result;
            }
            if Instant::now() >= deadline {
                result.message = "No time remained for the speed test. Increase the total timeout or reduce concurrent checks. Traffic quota is unknown.".into();
                return result;
            }
            match self.gate.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(10)),
                Err(TryLockError::Poisoned(_)) => {
                    result.message = "Could not start the speed test. Retry the check.".into();
                    return result;
                }
            }
        };
        if !control.acquire(settings.rate_limit, deadline) {
            result.outcome = if control.is_cancelled() {
                TransferOutcome::Cancelled
            } else {
                TransferOutcome::Skipped
            };
            result.message =
                "Speed test stopped while waiting to send the request. Traffic quota is unknown."
                    .into();
            return result;
        }
        let deadline = deadline.min(Instant::now() + Duration::from_secs(10));
        let mut url = match url::Url::parse(&self.url) {
            Ok(url) => url,
            Err(_) => {
                result.message = "The speed test endpoint is invalid.".into();
                return result;
            }
        };
        url.query_pairs_mut()
            .append_pair("bytes", &result.requested_bytes.to_string())
            .append_pair(
                "proxy_vouch_check",
                &format!(
                    "{}-{}",
                    chrono::Utc::now().timestamp_micros(),
                    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
                ),
            );
        let started = Instant::now();
        let mut easy = Easy2::new(Download::new(result.requested_bytes));
        let setup = (|| {
            configure_request(
                &mut easy,
                Some((proxy, protocol)),
                url.as_str(),
                settings,
                control,
                deadline,
            )?;
            // Compressed or HTML error responses must not inflate throughput.
            easy.accept_encoding("identity")?;
            easy.http_content_decoding(false)?;
            let mut headers = List::new();
            headers.append("Cache-Control: no-cache")?;
            headers.append("Pragma: no-cache")?;
            easy.http_headers(headers)
        })();
        if setup.is_err() {
            result.message = "The speed request could not be configured.".into();
            return result;
        }
        let multi = Multi::new();
        let Ok(mut handle) = multi.add2(easy) else {
            result.message = "The speed request could not be started.".into();
            return result;
        };
        let mut cancelled = false;
        let mut timed_out = false;
        let mut failed = false;
        loop {
            if control.is_cancelled() {
                cancelled = true;
                break;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            if multi.perform().is_err() {
                failed = true;
                break;
            }
            handle.get_mut().inspector.observe_tcp_connection();
            let mut completion = None;
            multi.messages(|message| {
                if let Some(value) = message.result_for2(&handle) {
                    completion = Some(value);
                }
            });
            if let Some(completion) = completion {
                if let Err(error) = completion {
                    timed_out = error.is_operation_timedout();
                    failed = true;
                }
                break;
            }
            if handle.get_ref().bytes >= result.requested_bytes {
                break;
            }
            if multi.wait(&mut [], Duration::from_millis(25)).unwrap_or(0) == 0 {
                thread::sleep(Duration::from_millis(2));
            }
        }
        let elapsed = started.elapsed();
        result.duration_ms = elapsed.as_millis() as u64;
        result.http_status = handle.response_code().ok().filter(|code| *code != 0);
        result.proxy_http_status = handle.http_connectcode().ok().filter(|code| *code != 0);
        let data = handle.get_ref();
        result.received_bytes = data.bytes;
        let valid_payload =
            result.http_status == Some(200) && data.valid_payload() && !data.invalid_payload;
        if valid_payload && data.bytes > 0 {
            result.download_mbps =
                Some(data.bytes as f64 * 8.0 / elapsed.as_secs_f64().max(0.000_001) / 1_000_000.0);
        }
        if cancelled {
            result.outcome = TransferOutcome::Cancelled;
            result.message = "The download was cancelled. The received sample does not determine a traffic quota.".into();
        } else if let Some((source, code)) = result
            .proxy_http_status
            .filter(|code| *code >= 400)
            .map(|code| ("proxy", code))
            .or_else(|| {
                result
                    .http_status
                    .filter(|code| *code != 200)
                    .map(|code| ("download endpoint", code))
            })
        {
            result.outcome = TransferOutcome::Failed;
            let signal = match code {
                413 => Some("size limit"),
                429 => Some("request rate limit"),
                509 => Some("bandwidth limit"),
                _ => None,
            };
            if let Some(signal) = signal {
                result.limit = TransferLimit::Signaled;
                result.message = format!("The {source} returned HTTP {code} ({signal} response). This does not establish the proxy account's total quota, remaining allowance, or reset time.");
            } else {
                result.message = format!("The {source} returned HTTP {code}. The speed sample was rejected; traffic quota is unknown.");
            }
        } else if data.invalid_payload
            || data.inspector.too_large
            || (result.http_status == Some(200) && !data.valid_payload())
        {
            result.outcome = TransferOutcome::Failed;
            result.download_mbps = None;
            result.message = "The endpoint returned an unexpected size, encoding or content type. No reliable speed or traffic quota could be determined.".into();
        } else if valid_payload && data.bytes == result.requested_bytes && !failed && !timed_out {
            result.outcome = TransferOutcome::Completed;
            result.limit = TransferLimit::NotObserved;
            result.message = "The selected sample was received in full. No transfer limit was observed within this sample; total traffic quota and remaining allowance are unknown.".into();
        } else {
            result.outcome = if data.bytes > 0 {
                TransferOutcome::Partial
            } else {
                TransferOutcome::Failed
            };
            result.message = if timed_out {
                "The download timed out. Try a smaller sample or a longer timeout; a timeout does not prove a traffic quota."
            } else {
                "The download was interrupted or incomplete. A connection failure does not prove a traffic quota."
            }.into();
        }
        result
    }
}

struct Download {
    inspector: Response,
    limit: u64,
    bytes: u64,
    status: u32,
    length: Option<u64>,
    binary: bool,
    encoded: bool,
    invalid_payload: bool,
    discarded: usize,
}

impl Download {
    fn new(limit: u64) -> Self {
        Self {
            inspector: Response::default(),
            limit,
            bytes: 0,
            status: 0,
            length: None,
            binary: false,
            encoded: false,
            invalid_payload: false,
            discarded: 0,
        }
    }
    fn valid_payload(&self) -> bool {
        self.binary && !self.encoded && self.length.is_none_or(|length| length == self.limit)
    }
}

impl Handler for Download {
    fn open_socket(
        &mut self,
        family: std::ffi::c_int,
        socktype: std::ffi::c_int,
        protocol: std::ffi::c_int,
    ) -> Option<curl_sys::curl_socket_t> {
        self.inspector.open_socket(family, socktype, protocol)
    }
    fn debug(&mut self, kind: InfoType, data: &[u8]) {
        self.inspector.debug(kind, data);
    }
    fn header(&mut self, data: &[u8]) -> bool {
        if !self.inspector.header(data) {
            return false;
        }
        let line = String::from_utf8_lossy(data);
        if line.starts_with("HTTP/") {
            self.status = line
                .split_whitespace()
                .nth(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            self.bytes = 0;
            self.length = None;
            self.binary = false;
            self.encoded = false;
        } else if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                self.length = value.parse().ok();
                self.invalid_payload |= self.length.is_none();
            } else if name.eq_ignore_ascii_case("content-type") {
                self.binary = value.split(';').next().is_some_and(|mime| {
                    mime.trim().eq_ignore_ascii_case("application/octet-stream")
                });
            } else if name.eq_ignore_ascii_case("content-encoding") {
                self.encoded = !value.eq_ignore_ascii_case("identity");
            }
        }
        true
    }
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        if self.status != 200 {
            self.discarded += data.len();
            return Ok(if self.discarded > 4096 { 0 } else { data.len() });
        }
        if !self.valid_payload() {
            self.invalid_payload = true;
            return Ok(0);
        }
        let accepted = (self.limit - self.bytes).min(data.len() as u64);
        self.bytes += accepted;
        if accepted != data.len() as u64 {
            self.invalid_payload = true;
            return Ok(0);
        }
        Ok(data.len())
    }
}

#[cfg(test)]
mod tests;
