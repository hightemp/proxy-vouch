use serde::{Deserialize, Serialize};
use std::{fmt, net::IpAddr};

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    pub code: String,
    pub message: String,
}

impl AppError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Auto,
    Http,
    Https,
    Socks4,
    Socks4a,
    Socks5,
    Socks5h,
}

impl Protocol {
    pub fn parse(value: &str) -> AppResult<Self> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            "socks4" => Ok(Self::Socks4),
            "socks4a" => Ok(Self::Socks4a),
            "socks5" => Ok(Self::Socks5),
            "socks5h" => Ok(Self::Socks5h),
            _ => Err(AppError::new(
                "UNSUPPORTED_PROTOCOL",
                "Use http, https, socks4, socks4a, socks5, socks5h or auto.",
            )),
        }
    }
    pub fn dns_mode(self) -> &'static str {
        match self {
            Self::Auto => "unknown",
            Self::Socks4 | Self::Socks5 => "local",
            _ => "remote",
        }
    }
    pub fn scheme(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Http => "http",
            Self::Https => "https",
            Self::Socks4 => "socks4",
            Self::Socks4a => "socks4a",
            Self::Socks5 => "socks5",
            Self::Socks5h => "socks5h",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials([redacted])")
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    pub protocol: Protocol,
    pub credentials: Option<Credentials>,
}

impl Proxy {
    pub fn address(&self) -> String {
        if self.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv6()) {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
    pub fn validate_auth(&self, protocol: Protocol) -> AppResult<()> {
        let Some(auth) = &self.credentials else {
            return Ok(());
        };
        let invalid = match protocol {
            Protocol::Socks4 | Protocol::Socks4a => auth.password.is_some(),
            Protocol::Socks5 | Protocol::Socks5h => {
                auth.username.is_empty()
                    || auth.username.len() > 255
                    || auth
                        .password
                        .as_ref()
                        .is_none_or(|p| p.is_empty() || p.len() > 255)
            }
            Protocol::Http | Protocol::Https => {
                auth.username.is_empty() || auth.username.contains(':') || auth.password.is_none()
            }
            Protocol::Auto => false,
        };
        if invalid {
            Err(AppError::new("UNSUPPORTED_AUTH", "Credentials cannot be represented by this protocol. Use a supported username/password pair or SOCKS4 USERID without a password."))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    Unchecked,
    Queued,
    Checking,
    Working,
    Failed,
    Inconclusive,
    Cancelled,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CheckSettings {
    pub url: String,
    pub fallback_url: String,
    pub ip_echo: bool,
    #[serde(default = "enabled_by_default")]
    pub country_lookup: bool,
    #[serde(default = "enabled_by_default")]
    pub anonymity_check: bool,
    #[serde(default = "enabled_by_default")]
    pub speed_check: bool,
    #[serde(default = "default_speed_test_mib")]
    pub speed_test_mib: u32,
    pub expected_status: u16,
    pub body_contains: String,
    pub concurrency: usize,
    pub rate_limit: u32,
    pub connect_timeout_ms: u64,
    pub attempt_timeout_ms: u64,
    pub total_timeout_ms: u64,
    pub retries: u8,
}

impl Default for CheckSettings {
    fn default() -> Self {
        Self {
            url: "https://api64.ipify.org?format=json".into(),
            fallback_url: String::new(),
            ip_echo: true,
            country_lookup: true,
            anonymity_check: true,
            speed_check: true,
            speed_test_mib: default_speed_test_mib(),
            expected_status: 200,
            body_contains: String::new(),
            concurrency: 20,
            rate_limit: 10,
            connect_timeout_ms: 3000,
            attempt_timeout_ms: 8000,
            total_timeout_ms: 45000,
            retries: 0,
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

fn default_speed_test_mib() -> u32 {
    1
}

impl CheckSettings {
    pub fn validate(&self) -> AppResult<()> {
        for value in [&self.url, &self.fallback_url]
            .into_iter()
            .filter(|v| !v.is_empty())
        {
            let parsed = url::Url::parse(value).map_err(|_| {
                AppError::new("INVALID_SETTINGS", "Enter a valid HTTP or HTTPS check URL.")
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.fragment().is_some()
            {
                return Err(AppError::new(
                    "INVALID_SETTINGS",
                    "Check URLs must use HTTP(S), without user information or fragments.",
                ));
            }
        }
        if self.url.is_empty()
            || !(1..=32).contains(&self.speed_test_mib)
            || !(1..=200).contains(&self.concurrency)
            || !(1..=100).contains(&self.rate_limit)
            || !(1000..=30000).contains(&self.connect_timeout_ms)
            || !(2000..=60000).contains(&self.attempt_timeout_ms)
            || !(5000..=300000).contains(&self.total_timeout_ms)
            || self.connect_timeout_ms > self.attempt_timeout_ms
            || self.attempt_timeout_ms > self.total_timeout_ms
            || self.retries > 2
            || !(100..=599).contains(&self.expected_status)
        {
            return Err(AppError::new("INVALID_SETTINGS", "Check the limits: connect timeout ≤ attempt timeout ≤ total timeout; concurrency 1–200; rate 1–100; retries 0–2; speed sample 1–32 MiB."));
        }
        Ok(())
    }
    pub fn safe_url(&self) -> String {
        safe_url(&self.url)
    }
}

pub fn safe_url(value: &str) -> String {
    match url::Url::parse(value) {
        Ok(mut url) => {
            url.set_query(None);
            url.set_fragment(None);
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Err(_) => "[invalid URL]".into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub protocol: Protocol,
    pub detected: Option<Protocol>,
    pub status: Status,
    pub authentication: String,
    pub code: String,
    pub stage: String,
    pub message: String,
    pub duration_ms: u64,
    pub exit_ip: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
    pub check_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnonymityResult {
    pub level: AnonymityLevel,
    pub message: String,
    pub check_url: String,
    pub observed_ip: Option<String>,
    pub proxy_headers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnonymityLevel {
    Transparent,
    Anonymous,
    Elite,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferOutcome {
    Completed,
    Partial,
    Failed,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferLimit {
    NotObserved,
    Signaled,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedResult {
    pub outcome: TransferOutcome,
    pub download_mbps: Option<f64>,
    pub requested_bytes: u64,
    pub received_bytes: u64,
    pub duration_ms: u64,
    pub limit: TransferLimit,
    pub http_status: Option<u32>,
    pub proxy_http_status: Option<u32>,
    pub check_url: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub status: Status,
    pub detected: Option<Protocol>,
    pub authentication: String,
    pub latency_ms: Option<u64>,
    pub total_duration_ms: u64,
    pub exit_ip: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub anonymity: Option<AnonymityResult>,
    #[serde(default)]
    pub speed: Option<SpeedResult>,
    pub checked_at: String,
    pub code: String,
    pub stage: String,
    pub message: String,
    pub check_url: String,
    pub attempts: Vec<Attempt>,
    #[serde(skip)]
    pub settings: CheckSettings,
}
