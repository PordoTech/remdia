
`indexed.mp4` is a project-owned, MIT-licensed 20-second synthetic 160x90/10fps
H.264 test pattern with one closed GOP per second, fragmented with a global SIDX.
Generated using the development FFmpeg fixture tool (`testsrc2`, `libopenh264`,
`-g 10 -bf 0`, and `+frag_keyframe+empty_moov+default_base_moof+global_sidx`). The
generator leaves SAP type unspecified; its SIDX entries were explicitly marked
type 1 for these known IDR-leading closed GOPs. Tests also clear one SAP entry and
alter the index scale to check preceding sync and independent media timescales.
No external video/account data is included. FFmpeg is not a runtime dependency.

`audio-tail.flac` reuses replayer's MIT-licensed four-second synthetic
`adapter.flac` tone (title QQ adapter test). It verifies system FLAC seeking
near the final partial block and repeated playback after EOF.

`scripts/flac-fixtures.py` generates four owned 30-minute mono/48 kHz/16-bit
constant-subframe files into `target/fixtures`: fixed 1024/4096 blocks, optional
SEEKTABLE, and variable 1024/4096/2048 blocks. Samples have predictable values.
The compact compressed files test timeline/seek correctness, not disk throughput.
Generation requires only Python's standard library; no external media library.
