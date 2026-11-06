# HTTP playback and indexed seeks

The compatibility demux uses the directly vendored Symphonia ISO MP4 reader in
`vendor/symphonia-format-isomp4`. This dependency is selected by the plugin's
manifest, so Git/path SDK consumers inherit it without a top-level Cargo patch.
Its MPL-2.0 license and modification record are included alongside the source.

MP4 packets retain separate PTS/DTS, signed/unsigned composition offsets and simple
edit origins. Fragment timestamps use TFDT rather than assuming every preceding
fragment has been read. Track media timescales remain independent of SIDX scales.

Direct SIDX media references map time to byte offsets. Coarse video seek selects
the preceding SAP type 1/2 fragment; ordinary MP4 uses STSS sync samples. The SDK
decodes preroll and suppresses output before the requested time. Missing indexes,
nested index references, unspecified SAP types or missing TFDT retain the bounded
packet keyframe search. This does not claim constant-time seeks for those layouts.
Interrupted readers are rebuilt; normal MP4 seeks and EOF recovery reuse movie
tables and the HTTP cache. Unsupported codecs retain the existing safe fallback.

`Request.network` defaults to 512-KiB Range requests, up to 64-KiB read-ahead per
read, and a 4-MiB LRU byte cache per source. Set `cache_bytes` to zero to disable
the cache. Range size is limited to 4 KiB–8 MiB, read-ahead to 1 byte–1 MiB and
cache size to 64 MiB. These source buffers are bounded independently of the shared
decoded-frame budget. Read-ahead fills on demand on the owning worker; no global
background worker or implicit thread pool is created. Pordo can still prefetch
provider responses independently of the SDK's decoding worker.

Content-Range start/end/length are checked. Strong ETag or Last-Modified validators
are sent through If-Range when available; entity changes fail rather than mixing
different versions. Cache hits still observe cancellation. Credentials, signed URL
renewal and provider selection belong to the application.

`Session::metrics()` reports read work time, seek reset time, emitted frames and
discarded preroll frames, including failed attempts in call/time totals. Read work
includes network waiting and graph/decoder execution; it does not measure GPU
upload, presentation, audio-device latency or end-to-end seek latency.

Regression coverage in `plugins/fallback-demux/src/tests.rs` compares indexed seek
packet contents/timestamps with sequential playback, checks skipped prefix reads,
different SIDX/media scales, preceding SAP, ordinary MP4 EOF reuse, HTTP cache
seeks, cancellation and disabled-cache byte preservation. Run:

```text
cargo test --locked -p remdia-backend-fallback-demux -p symphonia-format-isomp4
```

Multi-input worker coordination is still owned by Pordo. Extracting a generic
session group is a separate API change; clocks, presentation queues and UX buffer
thresholds must remain host policy.
