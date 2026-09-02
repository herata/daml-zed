//! LSP messages are `Content-Length: N\r\n\r\n` followed by N bytes of JSON.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Returns `Ok(None)` at a clean end of stream.
pub fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            len = rest.trim().parse().ok();
        }
    }
    let len = len.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "message without Content-Length")
    })?;
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

pub fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_one_message() {
        let mut input = &b"Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}"[..];
        let msg = read_message(&mut input).unwrap().unwrap();
        assert_eq!(msg["jsonrpc"], "2.0");
    }

    #[test]
    fn reads_two_messages_back_to_back() {
        let mut input =
            &b"Content-Length: 7\r\n\r\n{\"a\":1}Content-Length: 7\r\n\r\n{\"b\":2}"[..];
        assert_eq!(read_message(&mut input).unwrap().unwrap()["a"], 1);
        assert_eq!(read_message(&mut input).unwrap().unwrap()["b"], 2);
        assert!(read_message(&mut input).unwrap().is_none());
    }

    #[test]
    fn ignores_unknown_headers() {
        let mut input =
            &b"Content-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}"[..];
        assert_eq!(read_message(&mut input).unwrap().unwrap()["a"], 1);
    }

    #[test]
    fn round_trips() {
        let mut buf = Vec::new();
        write_message(&mut buf, &serde_json::json!({"a": 1})).unwrap();
        assert_eq!(read_message(&mut &buf[..]).unwrap().unwrap()["a"], 1);
    }
}
