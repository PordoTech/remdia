// Symphonia
// Copyright (c) 2019-2026 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Pordo modification: preserve signed/unsigned composition offsets (2026-11-06).
use crate::atoms::{Atom, AtomHeader, AtomIterator, ReadAtom, Result, decode_error};

#[derive(Debug)]
pub struct CttsAtom {
    entries: Vec<(u32, i64)>,
}
impl CttsAtom {
    pub fn offset(&self, sample: u32) -> i64 {
        self.entries
            .get(self.entries.partition_point(|(end, _)| *end <= sample))
            .map_or(0, |(_, offset)| *offset)
    }
}
fn composition_offset(version: u8, raw: u32) -> i64 {
    if version == 0 {
        i64::from(raw)
    } else {
        i64::from(raw as i32)
    }
}
impl Atom for CttsAtom {
    fn read<R: ReadAtom>(it: &mut AtomIterator<R>, _header: &AtomHeader) -> Result<Self> {
        let (version, _) = it.read_extended_header()?;
        if version > 1 {
            return decode_error("isomp4 (ctts): unsupported version");
        }
        let count = it.read_u32()?;
        let mut entries = Vec::new();
        let mut end = 0u32;
        for _ in 0..count {
            let samples = it.read_u32()?;
            end = match end.checked_add(samples) {
                Some(end) => end,
                None => return decode_error("isomp4 (ctts): sample count overflow"),
            };
            entries.push((end, composition_offset(version, it.read_u32()?)));
        }
        Ok(Self { entries })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_offsets_and_run_boundaries() {
        assert_eq!(composition_offset(0, 0xffff_ffff), 4_294_967_295);
        assert_eq!(composition_offset(1, 0xffff_fffe), -2);
        let table = CttsAtom {
            entries: vec![(2, 160), (3, -80), (5, 0)],
        };
        assert_eq!(
            (0..6).map(|i| table.offset(i)).collect::<Vec<_>>(),
            vec![160, 160, -80, 0, 0, 0]
        );
    }
}
