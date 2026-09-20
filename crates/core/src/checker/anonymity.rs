use super::{configure_request, CheckSettings, Control, Easy2, Multi, Protocol, Proxy, Response};
use crate::model::{safe_url, AnonymityLevel, AnonymityResult};
use curl::easy::List;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, TryLockError,
    },
    thread,
    time::{Duration, Instant},
};

// HTTP forwarding is deliberate: CONNECT encrypts request headers and hides
// headers added by an HTTP proxy. This is an observation of HTTP traffic only.
const JUDGE_URL: &str = "http://httpbingo.org/get";
const MARKERS: &[&str] = &[
    "via",
    "forwarded",
    "x-forwarded-for",
    "forwarded-for",
    "x-real-ip",
    "client-ip",
    "x-client-ip",
    "true-client-ip",
    "x-cluster-client-ip",
    "proxy-connection",
    "proxy-authorization",
    "x-proxy-id",
    "x-proxyuser-ip",
    "x-bluecoat-via",
    "x-forwarded-host",
    "x-original-forwarded-for",
];
static REQUEST_ID: AtomicU64 = AtomicU64::new(0);
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone)]
struct Observation {
    origins: Vec<IpAddr>,
    headers: BTreeMap<String, Vec<String>>,
}

pub(super) struct Lookup {
    url: String,
    baseline: Mutex<Option<Result<Observation>>>,
}

impl Default for Lookup {
    fn default() -> Self {
        Self::new(JUDGE_URL)
    }
}

impl Lookup {
    fn new(url: &str) -> Self {
        Self {
            url: url.into(),
            baseline: Mutex::new(None),
        }
    }

    pub(super) fn check(
        &self,
        proxy: &Proxy,
        protocol: Protocol,
        settings: &CheckSettings,
        control: &Control,
        deadline: Instant,
    ) -> AnonymityResult {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(10));
        let outcome = (|| {
            let baseline = self.reference(settings, control, deadline)?;
            let observed = request(
                Some((proxy, protocol)),
                &self.url,
                settings,
                control,
                deadline,
            )?;
            Ok(classify(&baseline, &observed, &self.url))
        })();
        outcome.unwrap_or_else(|message| {
            result(
                AnonymityLevel::Unknown,
                message,
                &self.url,
                None,
                Vec::new(),
            )
        })
    }

    fn reference(
        &self,
        settings: &CheckSettings,
        control: &Control,
        deadline: Instant,
    ) -> Result<Observation> {
        loop {
            stopped(control, deadline)?;
            match self.baseline.try_lock() {
                Ok(mut cached) => {
                    if let Some(reference) = &*cached {
                        return reference.clone();
                    }
                    let reference = request(None, &self.url, settings, control, deadline)
                        .and_then(|observed| {
                            if observed.origins.len() == 1 { Ok(observed) }
                            else { Err("The direct request returned multiple IPs; anonymity could not be determined.") }
                        })
                        .map_err(|_| "The direct IP reference is unavailable. Retry the check to measure anonymity.");
                    *cached = Some(reference.clone());
                    return reference;
                }
                Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(10)),
                Err(TryLockError::Poisoned(_)) => {
                    return Err("The anonymity reference could not be read. Start a new check.")
                }
            }
        }
    }
}

fn stopped(control: &Control, deadline: Instant) -> Result<()> {
    if control.is_cancelled() {
        Err("Anonymity lookup was cancelled; the completed availability result was kept.")
    } else if Instant::now() >= deadline {
        Err("No time remained for the anonymity lookup; the availability result was kept.")
    } else {
        Ok(())
    }
}

fn request(
    route: Option<(&Proxy, Protocol)>,
    endpoint: &str,
    settings: &CheckSettings,
    control: &Control,
    deadline: Instant,
) -> Result<Observation> {
    let deadline = deadline.min(Instant::now() + Duration::from_secs(5));
    stopped(control, deadline)?;
    if !control.acquire(settings.rate_limit.min(10), deadline) {
        return Err("The anonymity lookup was stopped while waiting to send a request.");
    }
    // Require an echoed request ID, so cached or unrelated JSON cannot be
    // mistaken for an observation of this request. It is not an authentication token.
    let nonce = format!(
        "{}-{}",
        chrono::Utc::now().timestamp_micros(),
        REQUEST_ID.fetch_add(1, Ordering::Relaxed)
    );
    let mut url = url::Url::parse(endpoint).map_err(|_| "The anonymity check URL is invalid.")?;
    url.query_pairs_mut()
        .append_pair("proxy_vouch_check", &nonce);
    let mut easy = Easy2::new(Response::default());
    configure_request(&mut easy, route, url.as_str(), settings, control, deadline)
        .map_err(|_| "The anonymity request could not be configured.")?;
    let mut headers = List::new();
    headers
        .append("Cache-Control: no-cache")
        .map_err(|_| "Could not configure request headers.")?;
    headers
        .append("Pragma: no-cache")
        .map_err(|_| "Could not configure request headers.")?;
    easy.http_headers(headers)
        .map_err(|_| "Could not configure request headers.")?;
    let multi = Multi::new();
    let mut handle = multi
        .add2(easy)
        .map_err(|_| "Could not start the anonymity request.")?;
    loop {
        stopped(control, deadline)?;
        multi
            .perform()
            .map_err(|_| "The anonymity request failed.")?;
        handle.get_mut().observe_tcp_connection();
        let mut completion = None;
        multi.messages(|message| {
            if let Some(value) = message.result_for2(&handle) {
                completion = Some(value);
            }
        });
        if let Some(completion) = completion {
            completion
                .map_err(|_| "The anonymity endpoint could not be reached. Retry the check.")?;
            break;
        }
        if multi.wait(&mut [], Duration::from_millis(25)).unwrap_or(0) == 0 {
            thread::sleep(Duration::from_millis(2));
        }
    }
    if handle.response_code().unwrap_or(0) != 200 || handle.get_ref().too_large {
        return Err("The anonymity endpoint returned an unsuccessful response. Retry the check.");
    }
    parse(&handle.get_ref().body, &nonce)
}

fn parse(body: &[u8], nonce: &str) -> Result<Observation> {
    const INVALID: &str = "The anonymity endpoint returned an invalid or cached response.";
    let json: Value = serde_json::from_slice(body).map_err(|_| INVALID)?;
    let echoed = &json["args"]["proxy_vouch_check"];
    if echoed.as_str() != Some(nonce)
        && !echoed
            .as_array()
            .is_some_and(|items| items.len() == 1 && items[0].as_str() == Some(nonce))
    {
        return Err(INVALID);
    }
    let origins = json["origin"]
        .as_str()
        .ok_or(INVALID)?
        .split(',')
        .map(|ip| {
            ip.trim()
                .parse::<IpAddr>()
                .map(normalize_ip)
                .map_err(|_| INVALID)
        })
        .collect::<Result<Vec<_>>>()?;
    if origins.is_empty() || origins.len() > 8 {
        return Err(INVALID);
    }
    let raw_headers = json["headers"].as_object().ok_or(INVALID)?;
    if raw_headers.is_empty() || raw_headers.len() > 128 {
        return Err(INVALID);
    }
    let mut headers = BTreeMap::new();
    for (name, value) in raw_headers {
        if !name.is_ascii() {
            return Err(INVALID);
        }
        let values = if let Some(value) = value.as_str() {
            vec![value.to_owned()]
        } else {
            value
                .as_array()
                .ok_or(INVALID)?
                .iter()
                .map(|value| value.as_str().map(str::to_owned).ok_or(INVALID))
                .collect::<Result<Vec<_>>>()?
        };
        headers
            .entry(name.to_ascii_lowercase())
            .or_insert_with(Vec::new)
            .extend(values);
    }
    if headers.get("user-agent").is_none_or(|values| {
        values.as_slice() != [concat!("ProxyVouch/", env!("CARGO_PKG_VERSION"))]
    }) {
        return Err(INVALID);
    }
    Ok(Observation { origins, headers })
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
}

fn ip_token(token: &str) -> Option<IpAddr> {
    token
        .parse::<IpAddr>()
        .ok()
        .or_else(|| token.parse::<SocketAddr>().ok().map(|address| address.ip()))
        .or_else(|| token.strip_prefix('[')?.strip_suffix(']')?.parse().ok())
        .map(normalize_ip)
}

fn tokens(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '=' | '"' | '\'' | '(' | ')'))
        .filter(|token| !token.is_empty())
}

fn signature(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| tokens(value))
        .map(|token| {
            if ip_token(token).is_some() {
                "[ip]".into()
            } else {
                token.to_ascii_lowercase()
            }
        })
        .collect()
}

fn result(
    level: AnonymityLevel,
    message: &str,
    url: &str,
    observed: Option<&Observation>,
    proxy_headers: Vec<String>,
) -> AnonymityResult {
    AnonymityResult {
        level,
        message: message.into(),
        check_url: safe_url(url),
        observed_ip: observed
            .filter(|o| o.origins.len() == 1)
            .map(|o| o.origins[0].to_string()),
        proxy_headers,
    }
}

fn classify(baseline: &Observation, observed: &Observation, url: &str) -> AnonymityResult {
    let direct_ip = baseline.origins[0];
    let leaks: Vec<_> = observed
        .headers
        .iter()
        .filter(|(_, values)| {
            values
                .iter()
                .flat_map(|value| tokens(value))
                .filter_map(ip_token)
                .any(|ip| ip == direct_ip)
        })
        .map(|(name, _)| name.clone())
        .collect();
    if observed.origins.contains(&direct_ip) || !leaks.is_empty() {
        return result(
            AnonymityLevel::Transparent,
            "The direct IP was visible in the HTTP response's origin or echoed request headers.",
            url,
            Some(observed),
            leaks,
        );
    }
    if observed
        .origins
        .iter()
        .any(|ip| ip.is_ipv4() != direct_ip.is_ipv4())
    {
        return result(AnonymityLevel::Unknown, "The direct and proxied requests used different IP families; anonymity could not be compared reliably.", url, Some(observed), Vec::new());
    }
    // A judge behind its own reverse proxy can add Via/X-Forwarded-For itself.
    // Compare against the direct observation, normalizing address values so the
    // expected change of exit IP alone is not treated as a proxy disclosure.
    let markers: Vec<_> = MARKERS
        .iter()
        .filter_map(|name| {
            let values = observed.headers.get(*name)?;
            if values.iter().all(|v| v.trim().is_empty()) {
                return None;
            }
            let new = baseline
                .headers
                .get(*name)
                .is_none_or(|baseline| signature(baseline) != signature(values));
            new.then(|| (*name).to_owned())
        })
        .collect();
    if !markers.is_empty() || observed.origins.len() > 1 {
        result(
            AnonymityLevel::Anonymous,
            "The direct IP was not observed, but the HTTP request exposed proxy indicators.",
            url,
            Some(observed),
            markers,
        )
    } else {
        result(AnonymityLevel::Elite, "The HTTP check exposed neither the measured direct IP nor additional proxy indicators. This is not a guarantee of anonymity for other traffic.", url, Some(observed), markers)
    }
}

#[cfg(test)]
mod tests;
