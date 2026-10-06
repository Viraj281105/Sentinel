//! A provider for a local Ollama server. Loopback only, plain HTTP, no proxy.
//!
//! Three separate things keep this off the network (docs/AI_POLICY.md, decision 1): the
//! URL is validated to be a literal loopback address, the HTTP client is built without TLS
//! support so no `https://` endpoint can work, and the client is told not to use any proxy
//! (an environment proxy would otherwise carry a "local" request elsewhere) and not to
//! follow redirects (a local server could otherwise bounce a request off the machine).

use std::net::Ipv4Addr;
use std::time::Duration;

use serde_json::{Value, json};

use crate::provider::{AiError, AiProvider, AiRequest};

const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// e.g. `http://127.0.0.1:11434`. Must be loopback.
    pub base_url: String,
    pub model: String,
    pub timeout: Duration,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:11434".to_owned(),
            model: "llama3.2".to_owned(),
            timeout: Duration::from_secs(60),
        }
    }
}

/// Accept only `http://` + a literal loopback host (`127.x.x.x`, `[::1]`, or `localhost`,
/// which is normalized to `127.0.0.1` so name resolution is never involved). Returns the
/// normalized base URL without a trailing slash.
pub fn validate_loopback_url(url: &str) -> Result<String, AiError> {
    let not_local = || AiError::NotLocal(url.to_owned());
    let trimmed = url.trim();
    let rest = trimmed
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("http://"))
        .map(|_| &trimmed[7..])
        .ok_or_else(not_local)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // No credentials, and nothing after the authority except an optional slash: this is a
    // base URL, not a request to some path.
    if authority.is_empty()
        || authority.contains('@')
        || !rest[authority.len()..].trim_matches('/').is_empty()
    {
        return Err(not_local());
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or_else(not_local)?;
        let port = match after {
            "" => None,
            a => Some(a.strip_prefix(':').ok_or_else(not_local)?),
        };
        (format!("[{h}]"), port)
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p)),
            None => (authority.to_owned(), None),
        }
    };
    if let Some(p) = port
        && p.parse::<u16>().is_err()
    {
        return Err(not_local());
    }
    let host = match host.to_lowercase().as_str() {
        "localhost" => "127.0.0.1".to_owned(),
        "[::1]" => "[::1]".to_owned(),
        h => match h.parse::<Ipv4Addr>() {
            Ok(ip) if ip.is_loopback() => ip.to_string(),
            _ => return Err(not_local()),
        },
    };
    Ok(match port {
        Some(p) => format!("http://{host}:{p}"),
        None => format!("http://{host}"),
    })
}

pub struct OllamaProvider {
    agent: ureq::Agent,
    endpoint: String,
    model: String,
}

impl OllamaProvider {
    pub fn new(cfg: &OllamaConfig) -> Result<Self, AiError> {
        let base = validate_loopback_url(&cfg.base_url)?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(cfg.timeout))
            .max_redirects(0)
            .proxy(None)
            .http_status_as_error(false)
            .build()
            .into();
        Ok(Self {
            agent,
            endpoint: format!("{base}/api/chat"),
            model: cfg.model.clone(),
        })
    }
}

fn map_transport(e: &ureq::Error) -> AiError {
    match e {
        ureq::Error::Timeout(_) => AiError::Timeout,
        other => AiError::Unavailable(other.to_string()),
    }
}

impl AiProvider for OllamaProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    fn complete(&self, req: &AiRequest) -> Result<String, AiError> {
        let mut body = json!({
            "model": self.model,
            "stream": false,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.user},
            ],
            // Deterministic and bounded.
            "options": {"temperature": 0, "num_predict": req.max_tokens},
        });
        if req.json_mode {
            body["format"] = json!("json");
        }
        let mut resp = self
            .agent
            .post(&self.endpoint)
            .send_json(&body)
            .map_err(|e| map_transport(&e))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) => AiError::TooLarge,
                other => map_transport(&other),
            })?;
        if status != 200 {
            return Err(AiError::BadResponse(format!(
                "the server answered HTTP {status}"
            )));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| AiError::BadResponse("the answer was not JSON".into()))?;
        v.get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| AiError::BadResponse("the answer had no message content".into()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn only_loopback_http_urls_are_accepted() {
        for ok in [
            "http://127.0.0.1:11434",
            "http://127.0.0.1:11434/",
            "http://localhost:11434",
            "HTTP://LOCALHOST:8080",
            "http://[::1]:11434",
            "http://127.5.5.5",
        ] {
            assert!(validate_loopback_url(ok).is_ok(), "{ok}");
        }
        assert_eq!(
            validate_loopback_url("http://localhost:11434/").unwrap(),
            "http://127.0.0.1:11434"
        );
        for bad in [
            "https://127.0.0.1:11434",
            "http://192.168.1.5:11434",
            "http://10.0.0.1",
            "http://example.com",
            "http://127.0.0.1.evil.com",
            "http://evil.com@127.0.0.1",
            "http://127.0.0.1@evil.com",
            "http://127.0.0.1:99999",
            "http://127.0.0.1:abc",
            "http://[::2]:11434",
            "http://0.0.0.0:11434",
            "http://localhost.evil.com",
            "http://",
            "127.0.0.1:11434",
            "ftp://127.0.0.1",
            "http://127.0.0.1/api/chat",
        ] {
            assert!(
                matches!(validate_loopback_url(bad), Err(AiError::NotLocal(_))),
                "{bad} must be refused"
            );
        }
    }

    /// A one-shot local HTTP server. Returns its base URL and the request it received.
    fn serve(response: String) -> (String, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(String::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = vec![0u8; 65536];
                let mut got = Vec::new();
                // Read until the body is complete (headers + content-length).
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).into_owned();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h]
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if got.len() >= h + 4 + len {
                            break;
                        }
                    }
                }
                *seen2.lock().unwrap() = String::from_utf8_lossy(&got).into_owned();
                let _ = s.write_all(response.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn cfg(url: &str) -> OllamaConfig {
        OllamaConfig {
            base_url: url.to_owned(),
            model: "test-model".into(),
            timeout: Duration::from_secs(5),
        }
    }

    fn req() -> AiRequest {
        AiRequest {
            system: "sys".into(),
            user: "hello".into(),
            json_mode: true,
            max_tokens: 100,
        }
    }

    fn http(status: &str, extra: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn talks_to_a_local_server_and_sends_a_bounded_deterministic_request() {
        let body = r#"{"message":{"role":"assistant","content":"{\"ok\":true}"},"done":true}"#;
        let (url, seen) = serve(http("200 OK", "Content-Type: application/json\r\n", body));
        let p = OllamaProvider::new(&cfg(&url)).unwrap();
        assert_eq!(p.id(), "ollama");
        assert_eq!(p.complete(&req()).unwrap(), r#"{"ok":true}"#);
        let sent = seen.lock().unwrap().clone();
        assert!(sent.starts_with("POST /api/chat "), "{sent}");
        let json: Value = serde_json::from_str(sent.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(json["model"], "test-model");
        assert_eq!(json["stream"], false);
        assert_eq!(json["format"], "json");
        assert_eq!(json["options"]["temperature"], 0);
        assert_eq!(json["options"]["num_predict"], 100);
        assert_eq!(json["messages"][1]["content"], "hello");
    }

    #[test]
    fn refuses_a_remote_endpoint_without_connecting() {
        let started = std::time::Instant::now();
        let err = OllamaProvider::new(&cfg("http://203.0.113.9:11434"))
            .err()
            .unwrap();
        assert!(matches!(err, AiError::NotLocal(_)));
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "it must not even try to connect"
        );
    }

    #[test]
    fn server_errors_and_junk_become_typed_errors() {
        let (url, _) = serve(http("500 Internal Server Error", "", "boom"));
        let err = OllamaProvider::new(&cfg(&url))
            .unwrap()
            .complete(&req())
            .unwrap_err();
        assert!(matches!(err, AiError::BadResponse(m) if m.contains("500")));

        let (url, _) = serve(http("200 OK", "", "not json"));
        let err = OllamaProvider::new(&cfg(&url))
            .unwrap()
            .complete(&req())
            .unwrap_err();
        assert!(matches!(err, AiError::BadResponse(_)));

        let (url, _) = serve(http("200 OK", "", "{}"));
        let err = OllamaProvider::new(&cfg(&url))
            .unwrap()
            .complete(&req())
            .unwrap_err();
        assert!(matches!(err, AiError::BadResponse(m) if m.contains("no message content")));
    }

    #[test]
    fn an_oversized_answer_is_refused() {
        let big = "x".repeat(2 * 1024 * 1024);
        let (url, _) = serve(http("200 OK", "", &big));
        let err = OllamaProvider::new(&cfg(&url))
            .unwrap()
            .complete(&req())
            .unwrap_err();
        assert!(matches!(err, AiError::TooLarge), "{err}");
    }

    #[test]
    fn a_redirect_is_not_followed() {
        let (url, _) = serve(http(
            "302 Found",
            "Location: http://203.0.113.9/steal\r\n",
            "",
        ));
        let err = OllamaProvider::new(&cfg(&url))
            .unwrap()
            .complete(&req())
            .unwrap_err();
        assert!(
            matches!(&err, AiError::BadResponse(m) if m.contains("302")),
            "a redirect must surface as an error: {err}"
        );
    }

    #[test]
    fn nothing_listening_is_unavailable_and_silence_times_out() {
        // A port that was just free is very unlikely to be open.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = OllamaProvider::new(&cfg(&format!("http://127.0.0.1:{port}")))
            .unwrap()
            .complete(&req())
            .unwrap_err();
        assert!(matches!(err, AiError::Unavailable(_)), "{err}");

        let silent = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = silent.local_addr().unwrap().port();
        let mut c = cfg(&format!("http://127.0.0.1:{port}"));
        c.timeout = Duration::from_millis(300);
        let err = OllamaProvider::new(&c)
            .unwrap()
            .complete(&req())
            .unwrap_err();
        assert!(matches!(err, AiError::Timeout), "{err}");
        drop(silent);
    }
}
