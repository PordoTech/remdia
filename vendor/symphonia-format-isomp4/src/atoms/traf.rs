// Symphonia
// Copyright (c) 2019-2026 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::atoms::{
    Atom, AtomHeader, AtomIterator, AtomType, ReadAtom, Result, TfhdAtom, TrunAtom, decode_error,
};

/// Track fragment atom.
#[allow(dead_code)]
#[derive(Debug)]
pub struct TrafAtom {
    /// Track fragment header.
    pub tfhd: TfhdAtom,
    pub decode_time: Option<u64>,
    /// Track fragment sample runs.
    pub truns: Vec<TrunAtom>,
    /// The total number of samples in this track fragment.
    pub total_sample_count: u32,
}

impl Atom for TrafAtom {
    fn read<R: ReadAtom>(it: &mut AtomIterator<R>, _header: &AtomHeader) -> Result<Self> {
        let mut tfhd = None;
        let mut decode_time = None;
        let mut truns = Vec::new();

        let mut total_sample_count = 0;

        while let Some(header) = it.next_header()? {
            match header.atom_type {
                AtomType::TrackFragmentHeader => {
                    tfhd = Some(it.read_atom::<TfhdAtom>()?);
                }
                AtomType::TrackFragmentDecodeTime => {
                    decode_time = Some(it.read_atom::<DecodeTime>()?.0);
                }
                AtomType::TrackFragmentRun => {
                    let trun = it.read_atom::<TrunAtom>()?;

                    // Increment the total sample count.
                    total_sample_count += trun.sample_count;

                    truns.push(trun);
                }
                _ => (),
            }
        }

        // Tfhd is mandatory.
        let Some(tfhd) = tfhd else {
            return decode_error("isomp4 (traf): missing tfhd atom");
        };

        Ok(TrafAtom {
            tfhd,
            decode_time,
            truns,
            total_sample_count,
        })
    }
}

struct DecodeTime(u64);
impl Atom for DecodeTime {
    fn read<R: ReadAtom>(it: &mut AtomIterator<R>, _header: &AtomHeader) -> Result<Self> {
        let (version, _) = it.read_extended_header()?;
        Ok(Self(match version {
            0 => u64::from(it.read_u32()?),
            1 => it.read_u64()?,
            _ => return decode_error("isomp4 (tfdt): invalid version"),
        }))
    }
}
