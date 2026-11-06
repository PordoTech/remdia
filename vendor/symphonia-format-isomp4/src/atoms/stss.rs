// Symphonia
// Copyright (c) 2019-2026 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::atoms::{Atom, AtomHeader, AtomIterator, ReadAtom, Result};

#[derive(Debug)]
pub struct StssAtom {
    pub samples: Vec<u32>,
}
impl StssAtom {
    pub fn preceding(&self, sample: u32) -> Option<u32> {
        self.samples
            .partition_point(|&s| s <= sample)
            .checked_sub(1)
            .map(|i| self.samples[i])
    }
}
impl Atom for StssAtom {
    fn read<R: ReadAtom>(it: &mut AtomIterator<R>, _header: &AtomHeader) -> Result<Self> {
        let (version, _) = it.read_extended_header()?;
        if version != 0 {
            return crate::atoms::decode_error("isomp4 (stss): invalid version");
        }
        let count = it.read_u32()?;
        let mut samples = Vec::new();
        for _ in 0..count {
            let Some(sample) = it.read_u32()?.checked_sub(1) else {
                return crate::atoms::decode_error("isomp4 (stss): zero sample");
            };
            if samples.last().is_some_and(|&s| s >= sample) {
                return crate::atoms::decode_error("isomp4 (stss): unsorted samples");
            }
            samples.push(sample);
        }
        Ok(Self { samples })
    }
}
