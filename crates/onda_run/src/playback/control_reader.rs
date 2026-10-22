use std::io::{self, BufRead};

use super::PlaybackControlRequest;

/// Keep incomplete records as bytes, including a UTF-8 character split by a timeout.
pub(super) struct ControlReader<R> {
    reader: R,
    pending: Vec<u8>,
}

impl<R: BufRead> ControlReader<R> {
    pub(super) fn new(reader: R) -> Self {
        Self {
            reader,
            pending: Vec::new(),
        }
    }

    pub(super) fn read_request(&mut self) -> io::Result<Option<PlaybackControlRequest>> {
        loop {
            if self.reader.read_until(b'\n', &mut self.pending)? == 0 && self.pending.is_empty() {
                return Ok(None);
            }
            if self.pending.iter().all(u8::is_ascii_whitespace) {
                self.pending.clear();
                continue;
            }
            let request = serde_json::from_slice(&self.pending)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            self.pending.clear();
            return request.map(Some);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{BufReader, Read};

    struct Chunks(VecDeque<io::Result<Vec<u8>>>);

    impl Read for Chunks {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            match self.0.pop_front() {
                Some(Ok(bytes)) => {
                    output[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                Some(Err(error)) => Err(error),
                None => Ok(0),
            }
        }
    }

    #[test]
    fn control_records_survive_timeouts_at_every_byte_boundary() {
        let record = "{\"command\":\"pause\",\"name\":\"café\"}\n".as_bytes();
        for split in 1..record.len() {
            for kind in [io::ErrorKind::TimedOut, io::ErrorKind::WouldBlock] {
                let chunks = Chunks(VecDeque::from([
                    Ok(record[..split].to_vec()),
                    Err(kind.into()),
                    Ok(record[split..].to_vec()),
                ]));
                let mut reader = ControlReader::new(BufReader::new(chunks));
                assert_eq!(reader.read_request().expect_err("timeout").kind(), kind);
                let request = reader
                    .read_request()
                    .expect("complete JSON")
                    .expect("record");
                assert_eq!(request.command, "pause");
                assert_eq!(request.name.as_deref(), Some("café"));
                assert!(reader.read_request().expect("EOF").is_none());
            }
        }
    }
}
