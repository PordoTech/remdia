# remdia MP4 timing and seek modifications

Based on the crates.io `symphonia-format-isomp4` 0.6.1 source, under MPL-2.0.
Upstream: https://github.com/pdeljanov/Symphonia

Changes dated 2026-11-06:

- Parse version 0 unsigned and version 1 signed `ctts` composition offsets.
- Preserve version 0/1 `trun` composition offsets for fragmented MP4.
- Keep packet DTS separate from PTS; interleave by DTS and apply composition
  offsets to PTS instead of assigning the decode timestamp to both fields.
- Apply the origin of a single normal-speed edit; complex edits retain the
  upstream behavior.

The decoder implementations are unchanged. This fixes B-frame picture/timestamp
associations when the existing remdia compatibility demux reads HTTP MP4 files.
The application-level regression compares complete decoded picture hashes and
their timestamps between the native local-file and HTTP demux paths.

Additional changes in remdia:

- Read TFDT fragment decode times, keeping SIDX and media timescales independent.
- Retain direct SIDX media references for byte/time seeking and preceding SAP.
- Parse STSS sync samples for ordinary MP4 coarse/keyframe seeks.
- Restart top-level atom scanning after random seeks and EOF without reparsing moov.
- Preserve the sequential path for unsupported/nested indexes and missing TFDT.

Changed source files: `atoms/ctts.rs`, `atoms/elst.rs`, `atoms/stbl.rs`,
`atoms/trun.rs`, `atoms/traf.rs`, `atoms/sidx.rs`, `atoms/stss.rs`, `atoms/mod.rs`,
`stream.rs`, and `demuxer.rs`. The plugin directly depends on this source; a
consumer-level Cargo patch is not needed. Replace it with an upstream version
when equivalent behavior passes the same timing, seeking and EOF regressions.
