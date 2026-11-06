use super::*;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use symphonia_core::{
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    units::Time,
};
use symphonia_format_isomp4::IsoMp4Reader;

struct Counted {
    cursor: std::io::Cursor<Vec<u8>>,
    reads: Arc<Mutex<Vec<u64>>>,
}
impl Read for Counted {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        self.reads.lock().unwrap().push(self.cursor.position());
        let n = out.len().min(256);
        self.cursor.read(&mut out[..n])
    }
}
impl Seek for Counted {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(to)
    }
}
impl MediaSource for Counted {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.cursor.get_ref().len() as u64)
    }
}
fn reader(bytes: &[u8], reads: Arc<Mutex<Vec<u64>>>) -> IsoMp4Reader<'static> {
    let mut stream = MediaSourceStream::new(
        Box::new(Counted {
            cursor: std::io::Cursor::new(bytes.to_vec()),
            reads,
        }),
        Default::default(),
    );
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).unwrap();
    IsoMp4Reader::try_new(stream, FormatOptions::default()).unwrap()
}
const INDEXED: &[u8] = include_bytes!("../../../tests/fixtures/indexed.mp4");
#[test]
fn indexed_seek_skips_prefix_matches_packets_and_reuses_after_eof() {
    let mut reference = reader(INDEXED, Arc::default());
    let mut packets = Vec::new();
    while let Some(p) = reference.next_packet().unwrap() {
        packets.push((p.pts, p.dts, p.data));
    }
    let reads = Arc::new(Mutex::new(Vec::new()));
    let mut format = reader(INDEXED, reads.clone());
    reads.lock().unwrap().clear();
    for time in [15.2, 3.4, 19.8] {
        format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from_micros((time * 1e6) as i64),
                    track_id: None,
                },
            )
            .unwrap();
        let p = format.next_packet().unwrap().unwrap();
        let expected = packets.iter().find(|(pts, _, _)| *pts == p.pts).unwrap();
        assert_eq!(p.dts, expected.1);
        assert_eq!(p.data, expected.2);
    }
    // A sequential header scan traverses hundreds of reads through preceding
    // fragments. Indexed seeks read only the three selected fragment headers/data.
    let offsets = reads.lock().unwrap();
    assert!(offsets.len() < 100, "too many reads: {}", offsets.len());
    assert!(offsets[0] > INDEXED.len() as u64 / 2);
    drop(offsets);
    while format.next_packet().unwrap().is_some() {}
    format
        .seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from_micros(2_500_000),
                track_id: None,
            },
        )
        .unwrap();
    assert!(format.next_packet().unwrap().is_some());
    format
        .seek(
            SeekMode::Coarse,
            SeekTo::Time {
                time: Time::from_micros(15_800_000),
                track_id: None,
            },
        )
        .unwrap();
    let p = format.next_packet().unwrap().unwrap();
    assert_eq!(
        format.tracks()[0]
            .time_base
            .unwrap()
            .calc_time(p.dts)
            .unwrap(),
        Time::from_micros(15_000_000)
    );
}

unsafe extern "C" fn poll(context: *mut std::ffi::c_void) -> u8 {
    u8::from(unsafe { &*context.cast::<AtomicBool>() }.load(Ordering::Acquire))
}
fn control(flag: &AtomicBool) -> Control {
    Control(remdia_plugin_abi::Control {
        context: (flag as *const AtomicBool).cast_mut().cast(),
        cancelled: poll,
    })
}
struct Server {
    url: String,
    calls: Arc<Mutex<Vec<u64>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(wrong_range: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let worker_calls = calls.clone();
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(500)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_millis(500)))
                    .unwrap();
                let mut headers = Vec::new();
                let mut b = [0; 1];
                while !headers.ends_with(b"\r\n\r\n") && headers.len() < 8192 {
                    if stream.read(&mut b).unwrap_or(0) == 0 {
                        break;
                    }
                    headers.push(b[0]);
                }
                let headers = String::from_utf8_lossy(&headers).to_ascii_lowercase();
                let Some(range) = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                else {
                    continue;
                };
                let (start, end) = range.split_once('-').unwrap();
                let start: u64 = start.parse().unwrap();
                let end: u64 = end.trim().parse::<u64>().unwrap().min(1024 * 1024 - 1);
                worker_calls.lock().unwrap().push(start);
                let first = start + u64::from(wrong_range);
                let body: Vec<_> = (start..=end).map(|i| (i % 251) as u8).collect();
                let _ = write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {first}-{end}/1048576\r\nContent-Length: {}\r\nETag: \"fixed\"\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        Self {
            url,
            calls,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
#[test]
fn network_cache_seeks_without_get_and_still_observes_cancellation() {
    let server = Server::new(false);
    let flag = AtomicBool::new(false);
    let mut request = Request::playback(&server.url);
    request.network.range_bytes = 64 * 1024;
    request.network.cache_bytes = 128 * 1024;
    let mut source = super::source::open(&request, control(&flag)).unwrap();
    let mut bytes = [0; 32768];
    source.read_exact(&mut bytes).unwrap();
    let count = server.calls.lock().unwrap().len();
    source.seek(SeekFrom::Start(100)).unwrap();
    source.read_exact(&mut bytes[..100]).unwrap();
    assert_eq!(
        &bytes[..100],
        &(100..200).map(|i| (i % 251) as u8).collect::<Vec<_>>()
    );
    assert_eq!(server.calls.lock().unwrap().len(), count);
    source.seek(SeekFrom::Start(900_000)).unwrap();
    source.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes[0], (900_000 % 251) as u8);
    flag.store(true, Ordering::Release);
    assert!(source.read(&mut bytes).is_err());
}
#[test]
fn network_without_cache_preserves_every_byte() {
    let server = Server::new(false);
    let flag = AtomicBool::new(false);
    let mut request = Request::playback(&server.url);
    request.network.cache_bytes = 0;
    let mut source = super::source::open(&request, control(&flag)).unwrap();
    for i in 0..300 {
        let mut byte = [0];
        source.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], (i % 251) as u8);
    }
}
#[test]
fn network_rejects_wrong_content_range() {
    let server = Server::new(true);
    let flag = AtomicBool::new(false);
    assert!(super::source::open(&Request::playback(server.url.clone()), control(&flag)).is_err());
}

#[test]
fn sidx_timescale_is_independent_and_non_sap_fragments_seek_to_preceding_sync() {
    let mut bytes = INDEXED.to_vec();
    let sidx = bytes.windows(4).position(|b| b == b"sidx").unwrap();
    let version = bytes[sidx + 4];
    assert_eq!(version, 1);
    let scale = u32::from_be_bytes(bytes[sidx + 12..sidx + 16].try_into().unwrap());
    bytes[sidx + 12..sidx + 16].copy_from_slice(&(scale * 2).to_be_bytes());
    // Version 1 SIDX has 64-bit earliest time and first offset.
    let count = u16::from_be_bytes(bytes[sidx + 34..sidx + 36].try_into().unwrap()) as usize;
    for i in 0..count {
        let at = sidx + 36 + i * 12;
        let duration = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap());
        bytes[at + 4..at + 8].copy_from_slice(&(duration * 2).to_be_bytes());
        if i == 15 {
            bytes[at + 8..at + 12].fill(0);
        }
    }
    let mut format = reader(&bytes, Arc::default());
    format
        .seek(
            SeekMode::Coarse,
            SeekTo::Time {
                time: Time::from_micros(15_800_000),
                track_id: None,
            },
        )
        .unwrap();
    let p = format.next_packet().unwrap().unwrap();
    assert_eq!(
        format.tracks()[0]
            .time_base
            .unwrap()
            .calc_time(p.dts)
            .unwrap(),
        Time::from_micros(14_000_000)
    );
}
#[test]
fn ordinary_mp4_sync_table_and_seek_after_eof_keep_reader_usable() {
    let mut format = reader(
        include_bytes!("../../../tests/fixtures/av.mp4"),
        Arc::default(),
    );
    while format.next_packet().unwrap().is_some() {}
    for time in [500_000, 0, 1_000_000, 200_000] {
        format
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: Time::from_micros(time),
                    track_id: None,
                },
            )
            .unwrap();
        assert!(format.next_packet().unwrap().is_some());
    }
}

#[test]
fn replay_zero_with_edit_origin_keeps_first_video_sync_packet() {
    let flag = AtomicBool::new(false);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/av.mp4");
    let request = Request::playback(path.to_string_lossy());
    let mut demux = super::demux::Demux::open(
        &NodeConfig {
            component: "fallback.demux".into(),
            settings: serde_json::to_value(request).unwrap(),
        },
        control(&flag),
    )
    .unwrap();
    let id = demux
        .info()
        .media
        .as_ref()
        .unwrap()
        .tracks
        .iter()
        .find(|t| t.kind == "video")
        .unwrap()
        .index as u32;
    let mut first = None;
    while let Receive::Output(packet) = demux.receive().unwrap() {
        if packet.header.stream_id == id && first.is_none() {
            first = Some((packet.header.info.pts_us(), packet.bytes().to_vec()));
        }
    }
    demux.reset(2, 0).unwrap();
    let replay = loop {
        match demux.receive().unwrap() {
            Receive::Output(packet) if packet.header.stream_id == id => {
                break (packet.header.info.pts_us(), packet.bytes().to_vec());
            }
            Receive::Output(_) => (),
            _ => panic!("video ended before first replay packet"),
        }
    };
    assert_eq!(Some(replay), first);
}
