//! Just enough RESP (the Redis protocol) for a client that sends commands from a fixed list:
//! splitting the list into commands, and finding where each reply ends. A reply is a success
//! unless it is an error (`-`), at any depth.

use crate::http::Parse;

/// Where the RESP value at the start of `b` ends, and whether it holds an error: `Some(Some((n,
/// error)))` when complete, `Some(None)` when more bytes are needed, `None` when malformed.
fn value(b: &[u8]) -> Option<Option<(usize, bool)>> {
    let Some(&kind) = b.first() else { return Some(None) };
    let Some(eol) = b.windows(2).position(|w| w == b"\r\n") else {
        return if b.len() > 64 * 1024 { None } else { Some(None) };
    };
    let line = &b[1..eol];
    let after = eol + 2;
    let number = || std::str::from_utf8(line).ok()?.parse::<i64>().ok();
    match kind {
        b'+' | b':' => Some(Some((after, false))),
        b'-' => Some(Some((after, true))),
        b'$' => {
            let n = number()?;
            if n < 0 {
                return Some(Some((after, false)));
            }
            let end = after + n as usize + 2;
            if b.len() < end {
                return Some(None);
            }
            (&b[end - 2..end] == b"\r\n").then_some(Some((end, false)))
        }
        b'*' => {
            let n = number()?;
            let (mut at, mut error) = (after, false);
            for _ in 0..n.max(0) {
                match value(&b[at..])? {
                    Some((len, e)) => {
                        at += len;
                        error |= e;
                    }
                    None => return Some(None),
                }
            }
            Some(Some((at, error)))
        }
        _ => None,
    }
}

/// Where the first reply in `buf` ends, as `http::parse` says it for a response: status 200
/// for a success, 500 for an error reply. RESP has no connection close in band.
pub fn parse(buf: &[u8]) -> Parse {
    match value(buf) {
        None => Parse::Bad,
        Some(None) => Parse::Incomplete,
        Some(Some((len, error))) => Parse::Done { status: if error { 500 } else { 200 }, len, close: false },
    }
}

/// Splits a file of RESP commands, written back to back, into the commands.
pub fn split(mut b: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    while !b.is_empty() {
        match value(b) {
            Some(Some((len, _))) => {
                out.push(b[..len].to_vec());
                b = &b[len..];
            }
            _ => return Err(format!("command {} is malformed or cut short", out.len() + 1)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        assert_eq!(parse(b"+OK\r\n+O"), Parse::Done { status: 200, len: 5, close: false });
        assert_eq!(parse(b"-ERR x\r\n"), Parse::Done { status: 500, len: 8, close: false });
        assert_eq!(parse(b"$-1\r\n"), Parse::Done { status: 200, len: 5, close: false });
        assert_eq!(parse(b"$3\r\nabc\r\n"), Parse::Done { status: 200, len: 9, close: false });
        assert_eq!(parse(b"$3\r\nab"), Parse::Incomplete);
        assert_eq!(parse(b"*2\r\n:1\r\n-E\r\n"), Parse::Done { status: 500, len: 12, close: false });
        assert_eq!(parse(b"*2\r\n:1\r\n"), Parse::Incomplete);
        assert_eq!(parse(b"?\r\n"), Parse::Bad);
    }

    #[test]
    fn commands() {
        let f = b"*2\r\n$3\r\nGET\r\n$1\r\nk\r\n*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$1\r\nv\r\n";
        let c = split(f).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], b"*2\r\n$3\r\nGET\r\n$1\r\nk\r\n");
        assert!(split(&f[..10]).is_err());
    }
}
