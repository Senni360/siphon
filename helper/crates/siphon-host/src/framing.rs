//! Bounded Chrome native-messaging frames; stdout is exclusively this transport.

use std::io::{Read, Write};
use std::sync::Mutex;

use serde_json::{Map, Value};

pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    /// The complete frame was consumed; the next header is safe to read.
    BadRequest(String),
    /// The stream cannot be resynchronized (truncated, oversized, or closed).
    Fatal(String),
}

fn read_exact(stream: &mut impl Read, buffer: &mut [u8]) -> Result<(), FrameError> {
    stream
        .read_exact(buffer)
        .map_err(|_| FrameError::Fatal("Truncated native messaging frame".into()))
}

/// Read one request object; `Ok(None)` at a clean end of input.
pub fn read_message(stream: &mut impl Read) -> Result<Option<Map<String, Value>>, FrameError> {
    let mut header = [0u8; 4];
    let mut filled = 0;
    while filled < 4 {
        match stream.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Fatal("Truncated native messaging frame".into())),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FrameError::Fatal(e.to_string())),
        }
    }
    let size = u32::from_le_bytes(header) as usize;
    if size > MAX_REQUEST_BYTES {
        return Err(FrameError::Fatal(
            "Native messaging request exceeds 16 MiB; cannot resynchronize".into(),
        ));
    }
    let mut payload = vec![0u8; size];
    read_exact(stream, &mut payload)?;
    if payload.is_empty() {
        return Err(FrameError::BadRequest(
            "Native messaging request cannot be empty".into(),
        ));
    }
    // serde_json rejects invalid UTF-8, NaN/Infinity and lone surrogate escapes.
    match serde_json::from_slice::<Value>(&payload) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        Ok(_) => Err(FrameError::BadRequest(
            "Native messaging request must be an object".into(),
        )),
        Err(_) => Err(FrameError::BadRequest("Invalid UTF-8 JSON request".into())),
    }
}

pub struct TooLarge;

pub struct Writer {
    inner: Mutex<(Box<dyn Write + Send>, bool)>,
}

impl Writer {
    pub fn new(stream: impl Write + Send + 'static) -> Self {
        Writer {
            inner: Mutex::new((Box::new(stream), false)),
        }
    }

    pub fn write(&self, message: &Value) -> Result<(), TooLarge> {
        let payload = serde_json::to_vec(message).expect("JSON values serialize");
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(TooLarge);
        }
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (stream, closed) = &mut *guard;
        if *closed {
            return Ok(());
        }
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&payload);
        if stream
            .write_all(&frame)
            .and_then(|_| stream.flush())
            .is_err()
        {
            *closed = true;
        }
        Ok(())
    }

    pub fn close(&self) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
    }

    pub fn closed(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(bytes: &[u8]) -> Vec<u8> {
        let mut out = (bytes.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(bytes);
        out
    }

    #[test]
    fn reads_objects_and_rejects_bad_frames_but_continues() {
        let mut input = Vec::new();
        for payload in [
            &b"{bad"[..],
            b"[]",
            b"\xff",
            b"",
            b"{\"x\":\"\\ud800\"}",
            b"{\"a\":NaN}",
            b"{\"id\":\"1\"}",
        ] {
            input.extend(frame(payload));
        }
        let mut stream = &input[..];
        for _ in 0..6 {
            assert!(matches!(
                read_message(&mut stream),
                Err(FrameError::BadRequest(_))
            ));
        }
        assert_eq!(read_message(&mut stream).ok().flatten().unwrap()["id"], "1");
        assert!(read_message(&mut stream).unwrap().is_none());
    }

    #[test]
    fn truncated_and_oversized_frames_are_fatal() {
        let mut stream = &[5u8, 0, 0, 0, b'{'][..];
        assert!(matches!(
            read_message(&mut stream),
            Err(FrameError::Fatal(_))
        ));
        let mut stream = &[0u8, 0, 0, 2][..];
        assert!(matches!(
            read_message(&mut stream),
            Err(FrameError::Fatal(_))
        ));
    }
}
