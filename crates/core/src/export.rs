use crate::{
    model::{AppError, AppResult, Protocol, Status},
    parser::bracket_host,
    session::Entry,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::Path,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub scope: String,
    pub format: String,
    pub credentials: bool,
    pub ids: Vec<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub text: String,
    pub count: usize,
    pub extension: String,
}

pub fn proxy_url(entry: &Entry, credentials: bool) -> AppResult<String> {
    let proxy = entry.parsed.proxy.as_ref().ok_or_else(|| {
        AppError::new(
            "INVALID_EXPORT",
            "Invalid records require Original lines or a report.",
        )
    })?;
    let protocol = entry
        .result
        .as_ref()
        .filter(|r| r.status == Status::Working)
        .and_then(|r| r.detected)
        .unwrap_or(proxy.protocol);
    if protocol == Protocol::Auto {
        return Err(AppError::new(
            "UNKNOWN_PROTOCOL",
            "Some records have no verified protocol. Choose Original lines or a report.",
        ));
    }
    let auth = if credentials {
        proxy
            .credentials
            .as_ref()
            .map(|auth| {
                let user = utf8_percent_encode(&auth.username, NON_ALPHANUMERIC);
                match &auth.password {
                    Some(pass) => {
                        format!("{user}:{}@", utf8_percent_encode(pass, NON_ALPHANUMERIC))
                    }
                    None => format!("{user}@"),
                }
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    Ok(format!(
        "{}://{auth}{}:{}",
        protocol.scheme(),
        bracket_host(&proxy.host),
        proxy.port
    ))
}

fn csv_safe(value: &str) -> String {
    if value.starts_with(['=', '+', '-', '@', '\t', '\r', '\n'])
        || value.trim_start().starts_with(['=', '+', '-', '@'])
    {
        format!("'{value}")
    } else {
        value.into()
    }
}

pub fn render(entries: &[Entry], options: &ExportOptions) -> AppResult<Payload> {
    let ids: HashSet<_> = options.ids.iter().copied().collect();
    let mut selected: Vec<_> = entries
        .iter()
        .filter(|e| match options.scope.as_str() {
            "Working" => e.status == Status::Working,
            "Failed" => e.status == Status::Failed,
            "Inconclusive" => e.status == Status::Inconclusive,
            "Checked" => matches!(
                e.status,
                Status::Working | Status::Failed | Status::Inconclusive
            ),
            "Selected" | "Filtered" => ids.contains(&e.id),
            "All" => true,
            _ => false,
        })
        .collect();
    let positions: HashMap<_, _> = options
        .ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();
    selected.sort_by_key(|e| positions.get(&e.id).copied().unwrap_or(usize::MAX));
    if selected.is_empty() {
        return Err(AppError::new(
            "EMPTY_EXPORT",
            "No records match this export scope.",
        ));
    }
    let count = selected.len();
    let (text, extension) = match options.format.as_str() {
        "urls" => (
            selected
                .iter()
                .map(|e| proxy_url(e, options.credentials))
                .collect::<AppResult<Vec<_>>>()?
                .join("\n")
                + "\n",
            "txt",
        ),
        "original" => {
            if !options.credentials
                && selected.iter().any(|e| {
                    e.parsed
                        .proxy
                        .as_ref()
                        .is_none_or(|p| p.credentials.is_some())
                })
            {
                return Err(AppError::new("CREDENTIALS_IN_SOURCE", "Original lines may contain credentials. Include credentials or choose a report."));
            }
            let schemas: HashSet<_> = selected
                .iter()
                .map(|e| (&e.parsed.header, e.parsed.delimiter))
                .collect();
            if schemas.len() > 1 {
                return Err(AppError::new(
                    "MIXED_SCHEMAS",
                    "The selection mixes source formats. Choose Proxy URLs or a report.",
                ));
            }
            let mut text = selected
                .first()
                .and_then(|e| e.parsed.header.clone())
                .map(|h| h + "\n")
                .unwrap_or_default();
            for entry in &selected {
                text.push_str(&entry.parsed.raw);
                text.push('\n');
            }
            let ext = match selected.first().and_then(|e| e.parsed.delimiter) {
                Some(b'\t') => "tsv",
                Some(_) => "csv",
                None => "txt",
            };
            (text, ext)
        }
        "compact" => {
            let mut lines = Vec::new();
            for entry in &selected {
                let _ = proxy_url(entry, options.credentials)?;
                let Some(proxy) = &entry.parsed.proxy else {
                    continue;
                };
                let protocol = entry
                    .result
                    .as_ref()
                    .filter(|r| r.status == Status::Working)
                    .and_then(|r| r.detected)
                    .unwrap_or(proxy.protocol);
                let mut line = proxy.address();
                if options.credentials {
                    if let Some(auth) = &proxy.credentials {
                        let invalid = auth.password.is_none()
                            || [&auth.username, auth.password.as_deref().unwrap_or_default()]
                                .iter()
                                .any(|s| {
                                    s.chars().any(|c| c.is_whitespace() || c == ':' || c == '@')
                                });
                        if invalid {
                            return Err(AppError::new("AMBIGUOUS_EXPORT", "These credentials need an encoded URI. Choose Proxy URLs or Original lines."));
                        }
                        line.push_str(&format!(
                            ":{}:{}",
                            auth.username,
                            auth.password.as_deref().unwrap_or_default()
                        ));
                    }
                }
                lines.push(format!("{line} {}", protocol.scheme()));
            }
            (lines.join("\n") + "\n", "txt")
        }
        "json" | "csv" => {
            let reports: Vec<_> = selected.iter().map(|e| {
                let proxy = e.parsed.proxy.as_ref(); let result = e.result.as_ref();
                let mut value = json!({"id":e.id,"source_name":e.parsed.source,"source_line":e.parsed.line,"label":e.parsed.label,"host":proxy.map(|p|&p.host),"port":proxy.map(|p|p.port),"requested_protocol":proxy.map(|p|p.protocol),"detected_protocol":result.and_then(|r|r.detected),"dns_mode":result.and_then(|r|r.detected).or_else(||proxy.map(|p|p.protocol)).map(Protocol::dns_mode),"authentication":result.map(|r|&r.authentication),"status":e.status,"error_code":e.parsed.error.as_ref().map(|err|&err.code).or_else(||result.map(|r|&r.code)),"error_stage":result.map(|r|&r.stage),"error_message":e.parsed.error.as_ref().map(|err|&err.message).or_else(||result.map(|r|&r.message)),"latency_ms":result.and_then(|r|r.latency_ms),"total_duration_ms":result.map(|r|r.total_duration_ms),"exit_ip":result.and_then(|r|r.exit_ip.as_ref()),"country_code":result.and_then(|r|r.country_code.as_ref()),"checked_at":result.map(|r|&r.checked_at),"profile":result.map(|r|if r.settings.ip_echo {"IP echo"} else {"Custom URL"}),"check_url":result.map(|r|&r.check_url)});
                if options.credentials {
                    value["username"] = json!(proxy.and_then(|p|p.credentials.as_ref()).map(|a|&a.username));
                    value["password"] = json!(proxy.and_then(|p|p.credentials.as_ref()).and_then(|a|a.password.as_ref()));
                }
                if options.format == "json" { value["attempts"] = json!(result.map(|r|&r.attempts)); }
                value
            }).collect();
            if options.format == "json" {
                (
                    serde_json::to_string_pretty(&json!({"schema_version":1,"records":reports}))
                        .map_err(|_| {
                            AppError::new("EXPORT_FAILED", "Could not serialize the report.")
                        })?
                        + "\n",
                    "json",
                )
            } else {
                let mut writer = csv::Writer::from_writer(Vec::new());
                let keys: Vec<_> = reports[0]
                    .as_object()
                    .map(|v| v.keys().cloned().collect())
                    .unwrap_or_default();
                writer
                    .write_record(&keys)
                    .map_err(|_| AppError::new("EXPORT_FAILED", "Could not write CSV headers."))?;
                for report in reports {
                    let fields = keys.iter().map(|key| {
                        let value = &report[key];
                        if value.is_null() {
                            String::new()
                        } else if let Some(text) = value.as_str() {
                            csv_safe(text)
                        } else {
                            value.to_string()
                        }
                    });
                    writer.write_record(fields).map_err(|_| {
                        AppError::new("EXPORT_FAILED", "Could not write CSV record.")
                    })?;
                }
                let bytes = writer.into_inner().map_err(|_| {
                    AppError::new("EXPORT_FAILED", "Could not finish the CSV report.")
                })?;
                (
                    String::from_utf8(bytes)
                        .map_err(|_| AppError::new("EXPORT_FAILED", "Could not encode CSV."))?,
                    "csv",
                )
            }
        }
        _ => {
            return Err(AppError::new(
                "INVALID_EXPORT",
                "Choose a supported export format.",
            ))
        }
    };
    Ok(Payload {
        text,
        count,
        extension: extension.into(),
    })
}

pub fn save_atomic(path: &Path, content: &str) -> AppResult<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| {
        AppError::new(
            "FILE_WRITE_FAILED",
            "Cannot create a temporary file in the selected folder.",
        )
    })?;
    temporary
        .write_all(content.as_bytes())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| {
            AppError::new(
                "FILE_WRITE_FAILED",
                "Could not write the file. Check free space and permissions.",
            )
        })?;
    temporary.persist(path).map_err(|_| {
        AppError::new(
            "FILE_WRITE_FAILED",
            "Could not replace the selected file. The previous file was preserved.",
        )
    })?;
    Ok(())
}
