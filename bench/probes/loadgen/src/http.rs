//! Just enough HTTP/1.1 for a keep-alive client that sends one fixed request: building the
//! request once, and finding where each response ends.

/// The whole request, headers and body, sent unchanged every time.
pub fn request(host: &str, path: &str, content_type: &str, headers: &[String], body: &[u8]) -> Vec<u8> {
    let mut head = format!("POST {path} HTTP/1.1\r\nHost: {host}\r\n");
    if !content_type.is_empty() {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    for h in headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut v = head.into_bytes();
    v.extend_from_slice(body);
    v
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parse {
    /// More bytes are needed.
    Incomplete,
    /// A whole response of `len` bytes is at the start of the buffer.
    Done { status: u16, len: usize, close: bool },
    /// Not a response this client can delimit (no length, or malformed).
    Bad,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn header_value<'a>(line: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    if line.len() > name.len() && line[..name.len()].eq_ignore_ascii_case(name) && line[name.len()] == b':' {
        Some(line[name.len() + 1..].trim_ascii())
    } else {
        None
    }
}

/// Where the first response in `buf` ends.
pub fn parse(buf: &[u8]) -> Parse {
    let Some(head_end) = find(buf, b"\r\n\r\n") else {
        return if buf.len() > 64 * 1024 { Parse::Bad } else { Parse::Incomplete };
    };
    let head = &buf[..head_end];
    let mut lines = head.split(|&b| b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l));
    let status_line = lines.next().unwrap_or(b"");
    if !status_line.starts_with(b"HTTP/1.") || status_line.len() < 12 {
        return Parse::Bad;
    }
    let Some(status) = std::str::from_utf8(&status_line[9..12]).ok().and_then(|s| s.parse::<u16>().ok()) else {
        return Parse::Bad;
    };
    let mut length = None;
    let mut chunked = false;
    let mut close = false;
    for line in lines {
        if let Some(v) = header_value(line, b"content-length") {
            match std::str::from_utf8(v).ok().and_then(|s| s.parse::<usize>().ok()) {
                Some(n) => length = Some(n),
                None => return Parse::Bad,
            }
        } else if let Some(v) = header_value(line, b"transfer-encoding") {
            chunked = v.to_ascii_lowercase().windows(7).any(|w| w == b"chunked");
        } else if let Some(v) = header_value(line, b"connection") {
            close = v.eq_ignore_ascii_case(b"close");
        }
    }
    let body = head_end + 4;
    if chunked {
        return match chunked_end(&buf[body..]) {
            Some(Some(n)) => Parse::Done { status, len: body + n, close },
            Some(None) => Parse::Incomplete,
            None => Parse::Bad,
        };
    }
    let n = match length {
        Some(n) => n,
        None if status == 204 || status == 304 || (100..200).contains(&status) => 0,
        None => return Parse::Bad,
    };
    if buf.len() >= body + n { Parse::Done { status, len: body + n, close } } else { Parse::Incomplete }
}

/// The length of a chunked body at the start of `b`, including the last chunk and any
/// trailers: `Some(Some(n))` when complete, `Some(None)` when more is needed, `None` when
/// malformed.
fn chunked_end(b: &[u8]) -> Option<Option<usize>> {
    let mut at = 0;
    loop {
        let Some(eol) = find(&b[at..], b"\r\n") else { return Some(None) };
        let line = &b[at..at + eol];
        let hex = line.split(|&c| c == b';').next().unwrap_or(b"").trim_ascii();
        let size = usize::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
        at += eol + 2;
        if size == 0 {
            // Trailers, if any, then an empty line.
            loop {
                let Some(eol) = find(&b[at..], b"\r\n") else { return Some(None) };
                at += eol + 2;
                if eol == 0 {
                    return Some(Some(at));
                }
            }
        }
        if b.len() < at + size + 2 {
            return Some(None);
        }
        if &b[at + size..at + size + 2] != b"\r\n" {
            return None;
        }
        at += size + 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_length() {
        let r = b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhelloHTTP/1.1";
        assert_eq!(parse(r), Parse::Done { status: 200, len: r.len() - 8, close: false });
        assert_eq!(parse(&r[..20]), Parse::Incomplete);
        assert_eq!(parse(&r[..r.len() - 10]), Parse::Incomplete);
    }

    #[test]
    fn chunked() {
        let r = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n2;x=y\r\nde\r\n0\r\n\r\n";
        assert_eq!(parse(r), Parse::Done { status: 200, len: r.len(), close: true });
        for cut in 60..r.len() {
            assert_eq!(parse(&r[..cut]), Parse::Incomplete, "cut at {cut}");
        }
        let t = b"HTTP/1.1 503 X\r\ntransfer-encoding: chunked\r\n\r\n0\r\nx-t: 1\r\n\r\n";
        assert_eq!(parse(t), Parse::Done { status: 503, len: t.len(), close: false });
    }

    #[test]
    fn undelimited() {
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\n\r\nbody"), Parse::Bad);
        assert_eq!(parse(b"HTTP/1.1 204 No Content\r\n\r\n"), Parse::Done { status: 204, len: 27, close: false });
        assert_eq!(parse(b"garbage\r\n\r\n"), Parse::Bad);
    }

    #[test]
    fn request_shape() {
        let r = request("h:1", "/p", "application/json", &["X-A: b".into()], b"{}");
        assert_eq!(
            r,
            b"POST /p HTTP/1.1\r\nHost: h:1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nX-A: b\r\n\r\n{}"
        );
    }
}
