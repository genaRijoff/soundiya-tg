//! Just enough HTTP/1.1 for the panel: one request per connection, small bounded heads and
//! bodies, and the checks that keep other sites and other networks out.
use std::net::IpAddr;

/// The request line and headers.
pub const MAX_HEAD: usize = 8 * 1024;
/// A command, a PIN or a search: never more.
pub const MAX_BODY: usize = 4 * 1024;
/// The session cookie.
pub const COOKIE: &str = "sid";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub host: String,
    pub origin: Option<String>,
    pub content_type: Option<String>,
    pub length: usize,
    /// The session cookie's value.
    pub session: Option<String>,
}

/// Parses the head (everything before the empty line).
pub fn parse_head(head: &str) -> Result<Request, &'static str> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target, version) = (first.next(), first.next(), first.next());
    let (Some(method), Some(target), Some(version), None) = (method, target, version, first.next())
    else {
        return Err("bad request line");
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err("bad request line");
    }
    let mut request = Request {
        method: method.to_owned(),
        // The panel's addresses take no query: it is dropped.
        path: target.split('?').next().unwrap_or("/").to_owned(),
        ..Request::default()
    };
    let mut hosts = 0;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or("bad header")?;
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => {
                hosts += 1;
                request.host = value.to_owned();
            }
            "origin" => request.origin = Some(value.to_owned()),
            "content-type" => request.content_type = Some(value.to_ascii_lowercase()),
            "content-length" => {
                request.length = value.parse().map_err(|_| "bad content-length")?;
            }
            // Bodies come with a length only: no chunked uploads to mis-frame.
            "transfer-encoding" => return Err("transfer-encoding is not accepted"),
            "cookie" => {
                if let Some(v) = value
                    .split(';')
                    .filter_map(|c| c.trim().split_once('='))
                    .find(|(k, _)| *k == COOKIE)
                    .map(|(_, v)| v.to_owned())
                {
                    request.session = Some(v);
                }
            }
            _ => {}
        }
    }
    if hosts != 1 {
        return Err("one Host header is needed");
    }
    Ok(request)
}

/// The panel answers only to the speaker's address or a local name: a page on another
/// site that makes its own name point at the speaker (DNS rebinding) is turned away.
pub fn host_allowed(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        // [fe80::1]:8765
        match rest.split_once(']') {
            Some((ip, port)) if port.is_empty() || port.strip_prefix(':').is_some_and(numeric) => {
                return ip.parse::<std::net::Ipv6Addr>().is_ok();
            }
            _ => return false,
        }
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) if numeric(port) => name,
            Some(_) => return false,
            None => host,
        }
    };
    if name.is_empty() || name.len() > 253 {
        return false;
    }
    if name.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    // "station", "station.local", "localhost": names only the home network gives out.
    let labels: Vec<&str> = lower.split('.').collect();
    labels.iter().all(|l| label_ok(l)) && (labels.len() == 1 || lower.ends_with(".local"))
}

fn numeric(port: &str) -> bool {
    !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit())
}

/// A change must come from the panel's own page.
pub fn origin_allowed(origin: Option<&str>, host: &str) -> bool {
    origin.is_some_and(|o| o.eq_ignore_ascii_case(&format!("http://{host}")))
}

/// Only the home network: private, link-local and loopback addresses.
pub fn peer_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return peer_allowed(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// Headers every answer carries: the page runs only its own script and loads covers from
/// SoundCloud's CDN; no framing, no referrer.
const SECURITY: &str = "Content-Security-Policy: default-src 'none'; script-src 'self'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data: https://*.sndcdn.com; connect-src 'self'; \
manifest-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n\
X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\n\
Cross-Origin-Resource-Policy: same-origin\r\n";

pub fn response(status: &str, content_type: &str, body: &[u8], extra: &str) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n{SECURITY}{extra}\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

/// The session cookie: this site only, not for scripts, a year at most.
pub fn session_cookie(token: &str) -> String {
    format!("Set-Cookie: {COOKIE}={token}; Path=/; Max-Age=15552000; HttpOnly; SameSite=Strict\r\n")
}
pub fn clear_cookie() -> String {
    format!("Set-Cookie: {COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_are_parsed_strictly() {
        let r = parse_head(
            "POST /api/cmd?x=1 HTTP/1.1\r\nHost: 192.168.1.5:8765\r\nOrigin: http://192.168.1.5:8765\r\n\
             Content-Type: application/json\r\nContent-Length: 12\r\nCookie: a=b; sid=abc123",
        )
        .unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/api/cmd");
        assert_eq!(r.host, "192.168.1.5:8765");
        assert_eq!(r.length, 12);
        assert_eq!(r.session.as_deref(), Some("abc123"));
        assert_eq!(r.content_type.as_deref(), Some("application/json"));
        for bad in [
            "GET / HTTP/1.1",                       // no Host
            "GET / HTTP/1.1\r\nHost: a\r\nHost: b", // two
            "GET http://x/ HTTP/1.1\r\nHost: a",    // absolute target
            "GET / HTTP/2\r\nHost: a",
            "GET  / HTTP/1.1\r\nHost: a",
            "POST / HTTP/1.1\r\nHost: a\r\nContent-Length: -1",
            "POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked",
            "GET / HTTP/1.1\r\nHost a",
        ] {
            assert!(parse_head(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_local_names_and_addresses_are_served() {
        for ok in [
            "192.168.1.5:8765",
            "192.168.1.5",
            "station:8765",
            "Yandex-Mini.local:8765",
            "localhost",
            "[fe80::1]:8765",
            "[::1]",
        ] {
            assert!(host_allowed(ok), "{ok}");
        }
        for bad in [
            "",
            "evil.example:8765",
            "station.lan:8765",
            "192.168.1.5:x",
            "a_b:8765",
            "[fe80::1",
            "[zz]:1",
            "x:123456",
        ] {
            assert!(!host_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn changes_only_from_the_panel_itself() {
        assert!(origin_allowed(
            Some("http://192.168.1.5:8765"),
            "192.168.1.5:8765"
        ));
        assert!(!origin_allowed(None, "192.168.1.5:8765"));
        assert!(!origin_allowed(
            Some("http://evil.example"),
            "192.168.1.5:8765"
        ));
        assert!(!origin_allowed(
            Some("https://192.168.1.5:8765"),
            "192.168.1.5:8765"
        ));
        assert!(!origin_allowed(Some("null"), "192.168.1.5:8765"));
    }

    #[test]
    fn only_the_home_network_may_connect() {
        for ok in [
            "192.168.0.10",
            "10.1.2.3",
            "172.20.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "fe80::1",
            "fd00::5",
            "::1",
            "::ffff:192.168.1.2",
        ] {
            assert!(peer_allowed(ok.parse().unwrap()), "{ok}");
        }
        for bad in [
            "8.8.8.8",
            "100.64.0.1",
            "2a00:1450::1",
            "::ffff:8.8.8.8",
            "172.32.0.1",
        ] {
            assert!(!peer_allowed(bad.parse().unwrap()), "{bad}");
        }
    }

    #[test]
    fn answers_carry_the_security_headers() {
        let r = String::from_utf8(response(
            "200 OK",
            "text/plain",
            b"hi",
            &session_cookie("t"),
        ))
        .unwrap();
        assert!(r.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(r.contains("Content-Length: 2\r\n"));
        assert!(r.contains("frame-ancestors 'none'"));
        assert!(r.contains("HttpOnly; SameSite=Strict"));
        assert!(r.ends_with("\r\n\r\nhi"));
    }
}
