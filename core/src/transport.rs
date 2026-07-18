//! HTTP transport to a Tasmota device's `/cm` command endpoint.
//!
//! Tasmota returns HTTP 200 even for rejected or unknown commands, signalling the
//! failure only in the JSON body. [`check_command_error`] enforces that command
//! success is read from the payload, never from the HTTP status code.

use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};

/// Optional web credentials for a device (Tasmota `WebPassword`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub user: String,
    pub password: String,
}

/// A device address plus optional credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAddr {
    /// IP address or hostname (no scheme).
    pub host: String,
    pub credentials: Option<Credentials>,
}

impl DeviceAddr {
    pub fn new(host: impl Into<String>) -> Self {
        DeviceAddr {
            host: host.into(),
            credentials: None,
        }
    }

    pub fn with_credentials(mut self, creds: Option<Credentials>) -> Self {
        self.credentials = creds;
        self
    }
}

/// The device interaction surface. Trait-based so the CLI and tests can inject a
/// fake without touching the network.
pub trait Transport {
    /// Send a Tasmota command (e.g. `Status 0`, `Power TOGGLE`) and return the
    /// parsed JSON body, having verified the device did not reject it.
    fn command(&self, addr: &DeviceAddr, cmnd: &str) -> Result<Value>;

    /// Download raw bytes from a device path (e.g. `/dl` config backup).
    fn download(&self, addr: &DeviceAddr, path: &str) -> Result<Vec<u8>>;

    /// Upload bytes as `multipart/form-data` to a device path (e.g. config
    /// restore). Returns the response body as a string value, since the device
    /// upload handler answers with HTML, not JSON.
    fn upload(
        &self,
        addr: &DeviceAddr,
        path: &str,
        field: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<Value>;
}

/// Percent-encode a query-string component, leaving only the unreserved set.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Detect a command-level failure reported in a 200 response body.
///
/// Tasmota answers an unknown command with `{"Command":"Unknown"}` and a missing
/// password with a `WARNING` about needing user+password.
pub fn check_command_error(value: &Value, cmnd: &str) -> Result<()> {
    if let Some(warning) = value.get("WARNING").and_then(Value::as_str) {
        let lower = warning.to_ascii_lowercase();
        if lower.contains("password") || lower.contains("user") {
            return Err(Error::Auth {
                message: format!("device requires authentication: {warning}"),
            });
        }
    }
    if let Some(cmd) = value.get("Command").and_then(Value::as_str)
        && (cmd.eq_ignore_ascii_case("unknown") || cmd.eq_ignore_ascii_case("error"))
    {
        return Err(Error::CommandRejected {
            command: cmnd.to_string(),
            message: format!("device reported Command={cmd}"),
        });
    }
    // A root-level error key (any case, e.g. {"Error":"Invalid parameter"}) is a
    // command failure even under HTTP 200.
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            if key.eq_ignore_ascii_case("error") {
                let detail = val
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| val.to_string());
                return Err(Error::CommandRejected {
                    command: cmnd.to_string(),
                    message: format!("device reported error: {detail}"),
                });
            }
        }
    }
    Ok(())
}

/// Build the `/cm` URL for a command.
fn command_url(addr: &DeviceAddr, cmnd: &str) -> String {
    let mut url = format!("http://{}/cm?cmnd={}", addr.host, encode_component(cmnd));
    if let Some(c) = &addr.credentials {
        url.push_str(&format!(
            "&user={}&password={}",
            encode_component(&c.user),
            encode_component(&c.password)
        ));
    }
    url
}

/// A real HTTP transport backed by `ureq`.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    timeout: Duration,
}

impl HttpTransport {
    pub fn new(timeout: Duration) -> Self {
        HttpTransport { timeout }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        HttpTransport::new(Duration::from_secs(5))
    }
}

impl Transport for HttpTransport {
    fn command(&self, addr: &DeviceAddr, cmnd: &str) -> Result<Value> {
        let url = command_url(addr, cmnd);
        let agent = ureq::AgentBuilder::new().timeout(self.timeout).build();
        let body = match agent.get(&url).call() {
            Ok(resp) => resp.into_string().map_err(|e| Error::Network {
                message: format!("reading response from {}: {e}", addr.host),
            })?,
            Err(ureq::Error::Status(401, _)) => {
                return Err(Error::Auth {
                    message: format!("{} returned HTTP 401 (WebPassword set?)", addr.host),
                });
            }
            Err(ureq::Error::Status(code, _)) => {
                return Err(Error::Network {
                    message: format!("{} returned HTTP {code}", addr.host),
                });
            }
            Err(ureq::Error::Transport(t)) => {
                return Err(Error::Network {
                    message: format!("{}: {t}", addr.host),
                });
            }
        };
        let value: Value = serde_json::from_str(&body).map_err(|_| Error::Parse {
            message: format!(
                "{} did not return JSON (not a Tasmota /cm endpoint?)",
                addr.host
            ),
        })?;
        check_command_error(&value, cmnd)?;
        Ok(value)
    }

    fn download(&self, addr: &DeviceAddr, path: &str) -> Result<Vec<u8>> {
        let mut url = format!("http://{}{}", addr.host, path);
        if let Some(c) = &addr.credentials {
            // The web UI download uses form login; passing credentials in the query
            // is a best effort for devices that accept it.
            url.push_str(&format!(
                "?user={}&password={}",
                encode_component(&c.user),
                encode_component(&c.password)
            ));
        }
        let agent = ureq::AgentBuilder::new().timeout(self.timeout).build();
        match agent.get(&url).call() {
            Ok(resp) => {
                let mut buf = Vec::new();
                resp.into_reader()
                    .read_to_end(&mut buf)
                    .map_err(|e| Error::Network {
                        message: format!("reading {} from {}: {e}", path, addr.host),
                    })?;
                Ok(buf)
            }
            Err(ureq::Error::Status(401, _)) => Err(Error::Auth {
                message: format!("{} returned HTTP 401 downloading {path}", addr.host),
            }),
            Err(ureq::Error::Status(code, _)) => Err(Error::Network {
                message: format!("{} returned HTTP {code} downloading {path}", addr.host),
            }),
            Err(ureq::Error::Transport(t)) => Err(Error::Network {
                message: format!("{}: {t}", addr.host),
            }),
        }
    }

    fn upload(
        &self,
        addr: &DeviceAddr,
        path: &str,
        field: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<Value> {
        let boundary = "----tasmotacli7f3aBoundary";
        let mut body: Vec<u8> = Vec::with_capacity(data.len() + 256);
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let mut url = format!("http://{}{}", addr.host, path);
        if let Some(c) = &addr.credentials {
            url.push_str(&format!(
                "?user={}&password={}",
                encode_component(&c.user),
                encode_component(&c.password)
            ));
        }
        let agent = ureq::AgentBuilder::new().timeout(self.timeout).build();
        match agent
            .post(&url)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body)
        {
            Ok(resp) => {
                let text = resp.into_string().unwrap_or_default();
                Ok(Value::String(text))
            }
            Err(ureq::Error::Status(401, _)) => Err(Error::Auth {
                message: format!("{} returned HTTP 401 uploading to {path}", addr.host),
            }),
            Err(ureq::Error::Status(code, _)) => Err(Error::Network {
                message: format!("{} returned HTTP {code} uploading to {path}", addr.host),
            }),
            Err(ureq::Error::Transport(t)) => Err(Error::Network {
                message: format!("{}: {t}", addr.host),
            }),
        }
    }
}

use std::io::Read;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_spaces_and_reserved() {
        assert_eq!(encode_component("Status 0"), "Status%200");
        assert_eq!(
            encode_component("Backlog Power ON; Delay 2"),
            "Backlog%20Power%20ON%3B%20Delay%202"
        );
        assert_eq!(encode_component("abc-_.~"), "abc-_.~");
    }

    #[test]
    fn command_url_includes_credentials() {
        let addr = DeviceAddr::new("192.0.2.10").with_credentials(Some(Credentials {
            user: "admin".into(),
            password: "p@ss".into(),
        }));
        let url = command_url(&addr, "Status 0");
        assert_eq!(
            url,
            "http://192.0.2.10/cm?cmnd=Status%200&user=admin&password=p%40ss"
        );
    }

    #[test]
    fn unknown_command_is_rejected() {
        let v = json!({"Command": "Unknown"});
        let err = check_command_error(&v, "Frobnicate").unwrap_err();
        assert_eq!(err.kind(), "command_rejected");
    }

    #[test]
    fn password_warning_is_auth_error() {
        let v = json!({"WARNING": "Need user+password to send command"});
        let err = check_command_error(&v, "Power TOGGLE").unwrap_err();
        assert_eq!(err.kind(), "auth");
    }

    #[test]
    fn root_error_key_is_rejected() {
        assert_eq!(
            check_command_error(&json!({"Error": "Invalid parameter"}), "X")
                .unwrap_err()
                .kind(),
            "command_rejected"
        );
        assert_eq!(
            check_command_error(&json!({"ERROR": "boom"}), "X")
                .unwrap_err()
                .kind(),
            "command_rejected"
        );
    }

    #[test]
    fn ok_response_passes() {
        let v = json!({"POWER": "ON"});
        assert!(check_command_error(&v, "Power ON").is_ok());
    }
}
