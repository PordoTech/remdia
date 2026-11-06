use anyhow::{Context, Result, ensure};
use remdia_graph::{Control, Request};
use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    time::{Duration, Instant},
};
use symphonia_core::io::MediaSource;

struct Local {
    file: std::fs::File,
    control: Control,
}
fn interrupted(error: anyhow::Error) -> io::Error {
    io::Error::other(error.to_string())
}
impl Read for Local {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.control.check().map_err(interrupted)?;
        self.file.read(out)
    }
}
impl Seek for Local {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.control.check().map_err(interrupted)?;
        self.file.seek(to)
    }
}
impl MediaSource for Local {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.file.metadata().ok().map(|m| m.len())
    }
}
struct Chunk {
    start: u64,
    bytes: Vec<u8>,
}
#[derive(Default)]
struct Cache {
    chunks: VecDeque<Chunk>,
    bytes: usize,
    limit: usize,
}
impl Cache {
    fn read(&mut self, at: u64, out: &mut [u8]) -> usize {
        let Some(i) = self
            .chunks
            .iter()
            .rposition(|c| at >= c.start && at - c.start < c.bytes.len() as u64)
        else {
            return 0;
        };
        let c = self.chunks.remove(i).unwrap();
        let start = (at - c.start) as usize;
        let n = out.len().min(c.bytes.len() - start);
        out[..n].copy_from_slice(&c.bytes[start..start + n]);
        self.chunks.push_back(c);
        n
    }
    fn insert(&mut self, start: u64, bytes: Vec<u8>) {
        if bytes.len() > self.limit {
            return;
        }
        while self.bytes + bytes.len() > self.limit {
            if let Some(c) = self.chunks.pop_front() {
                self.bytes -= c.bytes.len();
            }
        }
        self.bytes += bytes.len();
        self.chunks.push_back(Chunk { start, bytes });
    }
}
struct Network {
    client: reqwest::blocking::Client,
    url: String,
    position: u64,
    length: Option<u64>,
    seekable: bool,
    response: Option<reqwest::blocking::Response>,
    control: Control,
    timeout: Duration,
    range_bytes: usize,
    read_ahead_bytes: usize,
    response_position: u64,
    response_end: Option<u64>,
    validator: Option<String>,
    cache: Cache,
}
impl Network {
    fn connect(&mut self) -> io::Result<()> {
        let started = Instant::now();
        let mut request_retries = 0;
        loop {
            self.control.check().map_err(interrupted)?;
            if started.elapsed() > self.timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "HTTP response timed out",
                ));
            }
            let end = self.position.saturating_add(self.range_bytes as u64 - 1);
            let mut request = self
                .client
                .get(&self.url)
                .header(reqwest::header::ACCEPT_ENCODING, "identity")
                .header(
                    reqwest::header::RANGE,
                    format!("bytes={}-{end}", self.position),
                );
            if let Some(validator) = &self.validator {
                request = request.header(reqwest::header::IF_RANGE, validator);
            }
            let result = request.send();
            let response = match result {
                Ok(r) => r,
                Err(e) if e.is_timeout() || e.is_connect() => continue,
                // A peer can close after accepting our GET but before returning
                // headers. Retrying this idempotent request is safe: position
                // advances only after body bytes are admitted by Read below.
                Err(e) if e.is_request() && request_retries < 3 => {
                    request_retries += 1;
                    continue;
                }
                Err(e) => {
                    let detail = anyhow::Error::new(e);
                    return Err(io::Error::other(format!("{detail:#}")));
                }
            };
            if response.status().as_u16() == 416 && self.length.is_some_and(|l| self.position >= l)
            {
                self.response = None;
                return Ok(());
            }
            if !response.status().is_success() {
                return Err(io::Error::other(format!("HTTP {}", response.status())));
            }
            if response.status().as_u16() == 206 {
                let range = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| io::Error::other("missing Content-Range"))?;
                let (span, total) = range
                    .strip_prefix("bytes ")
                    .and_then(|s| s.split_once('/'))
                    .ok_or_else(|| io::Error::other("invalid Content-Range"))?;
                let (first, last) = span
                    .split_once('-')
                    .and_then(|(s, e)| Some((s.parse::<u64>().ok()?, e.parse::<u64>().ok()?)))
                    .ok_or_else(|| io::Error::other("invalid Content-Range start"))?;
                if first != self.position || last < first || last > end {
                    return Err(io::Error::other("server returned the wrong byte range"));
                }
                self.length = if total == "*" {
                    None
                } else {
                    Some(
                        total
                            .parse::<u64>()
                            .map_err(|_| io::Error::other("invalid range length"))?,
                    )
                };
                if self.length.is_some_and(|len| last >= len) {
                    return Err(io::Error::other("range exceeds source length"));
                }
                self.response_end = last.checked_add(1);
                self.seekable = true;
            } else {
                if self.position != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "source does not support HTTP Range seeking",
                    ));
                }
                self.length = response.content_length();
                self.response_end = self.length;
                self.seekable = false;
            }
            let validator = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.starts_with("W/"))
                .or_else(|| {
                    response
                        .headers()
                        .get(reqwest::header::LAST_MODIFIED)
                        .and_then(|v| v.to_str().ok())
                })
                .map(str::to_owned);
            if self
                .validator
                .as_ref()
                .zip(validator.as_ref())
                .is_some_and(|(a, b)| a != b)
            {
                return Err(io::Error::other("HTTP entity changed during playback"));
            }
            if validator.is_some() {
                self.validator = validator;
            }
            self.response_position = self.position;
            self.response = Some(response);
            return Ok(());
        }
    }
}
impl Read for Network {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let started = Instant::now();
        loop {
            self.control.check().map_err(interrupted)?;
            if self.length.is_some_and(|l| self.position >= l) {
                return Ok(0);
            }
            if started.elapsed() > self.timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "HTTP body timed out",
                ));
            }
            let cached = self.cache.read(self.position, out);
            if cached > 0 {
                self.position += cached as u64;
                return Ok(cached);
            }
            if self.response_position != self.position || self.response_end == Some(self.position) {
                self.response = None;
            }
            if self.response.is_none() {
                self.connect()?;
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(0);
            };
            let mut bytes = vec![0; self.read_ahead_bytes.max(out.len().min(64 * 1024))];
            if self.cache.limit < bytes.len() {
                bytes.resize(out.len().min(self.read_ahead_bytes), 0);
            }
            match response.read(&mut bytes) {
                Ok(0) => {
                    self.response = None;
                    if !self.seekable {
                        self.length = Some(self.position);
                        return Ok(0);
                    }
                }
                Ok(n) => {
                    self.control.check().map_err(interrupted)?;
                    if self
                        .response_end
                        .is_some_and(|end| self.position.saturating_add(n as u64) > end)
                    {
                        return Err(io::Error::other("HTTP body exceeds declared range"));
                    }
                    bytes.truncate(n);
                    let delivered = n.min(out.len());
                    out[..delivered].copy_from_slice(&bytes[..delivered]);
                    self.response_position = self
                        .position
                        .checked_add(n as u64)
                        .ok_or_else(|| io::Error::other("HTTP offset overflow"))?;
                    self.cache.insert(self.position, bytes);
                    self.position += delivered as u64;
                    // With caching disabled, do not consume bytes that cannot be delivered.
                    return Ok(delivered);
                }
                Err(e) => {
                    self.response = None;
                    if !self.seekable {
                        return Err(e);
                    }
                }
            }
        }
    }
}
impl Seek for Network {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.control.check().map_err(interrupted)?;
        let position = match to {
            SeekFrom::Start(s) => i128::from(s),
            SeekFrom::Current(s) => i128::from(self.position) + i128::from(s),
            SeekFrom::End(s) => {
                i128::from(self.length.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Unsupported, "unknown source length")
                })?) + i128::from(s)
            }
        };
        let position = u64::try_from(position)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid seek"))?;
        if position == self.position {
            return Ok(position);
        }
        if !self.seekable && position != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "source is not seekable",
            ));
        }
        self.position = position;
        // Keep the response until a cache miss proves a new Range is needed.
        Ok(position)
    }
}
impl MediaSource for Network {
    fn is_seekable(&self) -> bool {
        self.seekable
    }
    fn byte_len(&self) -> Option<u64> {
        self.length
    }
}
pub fn open(request: &Request, control: Control) -> Result<Box<dyn MediaSource>> {
    control.check()?;
    if request.source.starts_with("http://") || request.source.starts_with("https://") {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(500))
            .connect_timeout(Duration::from_millis(300))
            .build()?;
        let mut source = Network {
            client,
            url: request.source.clone(),
            position: 0,
            length: None,
            seekable: false,
            response: None,
            control,
            timeout: Duration::from_millis(request.timeout_ms),
            range_bytes: request.network.range_bytes,
            read_ahead_bytes: request.network.read_ahead_bytes,
            response_position: 0,
            response_end: None,
            validator: None,
            cache: Cache {
                limit: request.network.cache_bytes,
                ..Cache::default()
            },
        };
        source
            .connect()
            .with_context(|| format!("open HTTP source {}", request.source))?;
        control.check()?;
        Ok(Box::new(source))
    } else {
        ensure!(
            !request.source.contains("://"),
            "unsupported source protocol"
        );
        Ok(Box::new(Local {
            file: std::fs::File::open(&request.source)
                .with_context(|| format!("open {}", request.source))?,
            control,
        }))
    }
}
