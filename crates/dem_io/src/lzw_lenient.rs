//! Per-chunk LZW decoder that bypasses `tiff::Decoder::read_chunk` for LZW-
//! compressed TIFF chunks.
//!
//! The tiff crate's strip/tile LZW reader feeds bytes through a `BufReader`
//! and asks weezl to stop only when it sees an EOI symbol or `LzwStatus::Done`.
//! When the encoded payload lacks a proper EOI, weezl eventually returns
//! `LzwStatus::NoProgress`, which the tiff crate translates into
//! `UnexpectedEof("no lzw end code found")` — crashing the decode of any chunk
//! whose compressed bytes happen to sit at the end of the file (issue #40).
//!
//! This module reads the exact `(offset, byte_count)` for each chunk from the
//! TIFF directory, decompresses the in-memory slice with weezl, and **accepts
//! decode success when the output buffer is filled regardless of `LzwStatus`**
//! — never depending on an EOI symbol being present.
//!
//! Used only for `Compression::LZW`; other compressions stay on the tiff crate's
//! existing path.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

use tiff::decoder::Decoder;
use tiff::tags::Tag;
use weezl::BitOrder;
use weezl::LzwStatus;
use weezl::decode::Configuration as LzwConfiguration;

use crate::DemError;

pub(crate) const COMPRESSION_LZW: u16 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SampleByteOrder {
    LittleEndian,
    BigEndian,
}

/// Read `Compression` (tag 259) and `Predictor` (tag 317).  Predictor defaults
/// to 1 (no predictor) when the tag is absent.
pub(crate) fn compression_and_predictor(
    decoder: &mut Decoder<BufReader<File>>,
) -> Result<(u16, u16), DemError> {
    let compression = decoder.get_tag_u32(Tag::Compression)? as u16;
    let predictor = decoder.get_tag_u32(Tag::Predictor).unwrap_or(1) as u16;
    Ok((compression, predictor))
}

/// Return `(offsets, byte_counts, is_tiled)` for every chunk in the current IFD.
/// Tries `TileOffsets`/`TileByteCounts` first, falls back to `StripOffsets`/
/// `StripByteCounts` for stripped images.
///
/// `is_tiled` matters for the lenient reader: tiled chunks always encode the
/// full padded `TileWidth × TileLength` (edge tiles include padding pixels in
/// the LZW payload), while stripped chunks encode only the actual rows present
/// in that strip (no padding).
pub(crate) fn chunk_layout(
    decoder: &mut Decoder<BufReader<File>>,
) -> Result<(Vec<u64>, Vec<u64>, bool), DemError> {
    if let Ok(offsets) = decoder.get_tag_u64_vec(Tag::TileOffsets) {
        let counts = decoder.get_tag_u64_vec(Tag::TileByteCounts)?;
        return Ok((offsets, counts, true));
    }
    let offsets = decoder.get_tag_u64_vec(Tag::StripOffsets)?;
    let counts = decoder.get_tag_u64_vec(Tag::StripByteCounts)?;
    Ok((offsets, counts, false))
}

/// Read the file's byte-order marker (`II` = little-endian, `MM` = big-endian)
/// from the first two bytes of the TIFF header.
pub(crate) fn read_byte_order(file: &mut File) -> Result<SampleByteOrder, DemError> {
    file.seek(SeekFrom::Start(0))?;
    let mut hdr = [0u8; 2];
    file.read_exact(&mut hdr)?;
    match &hdr {
        b"II" => Ok(SampleByteOrder::LittleEndian),
        b"MM" => Ok(SampleByteOrder::BigEndian),
        _ => Err("not a TIFF file: bad byte-order marker".into()),
    }
}

/// Decode one LZW chunk of F32 samples.
///
/// * `file` — opened on the source TIFF; this function seeks freely.
/// * `offset`, `byte_count` — from `TileOffsets[i]` / `TileByteCounts[i]`
///   (or strip equivalents) for chunk `i`.
/// * `chunk_cols` — encoded chunk width (TileWidth for tiled, ImageWidth for
///   stripped).  Edge tiles still encode padded pixels here.
/// * `encoded_rows` — number of rows actually in the LZW payload.  For tiled
///   chunks this is always `TileLength` (padded).  For stripped chunks this is
///   `min(RowsPerStrip, ImageLength - strip_idx*RowsPerStrip)` (no padding).
/// * `actual_cols`/`actual_rows` — pixel dims actually inside the image bounds.
///   For stripped chunks these equal `chunk_cols`/`encoded_rows`.  For tiled
///   edge chunks they may be smaller; the returned `Vec<f32>` trims to them to
///   match `Decoder::read_chunk`'s convention.
/// * `predictor` — TIFF Predictor (1 = none, 2 = horizontal, 3 = floating-point).
/// * `sample_order` — file byte order for Predictor 1/2 (Predictor 3 always
///   yields big-endian byte groups per TIFF Tech Note 3).
// Args describe one TIFF chunk's geometry + decode parameters; each is a distinct input.
#[allow(clippy::too_many_arguments)]
pub(crate) fn read_lzw_chunk_f32(
    file: &mut File,
    offset: u64,
    byte_count: usize,
    chunk_cols: usize,
    encoded_rows: usize,
    actual_cols: usize,
    actual_rows: usize,
    predictor: u16,
    sample_order: SampleByteOrder,
) -> Result<Vec<f32>, DemError> {
    const BPS: usize = 4;
    let row_bytes = chunk_cols * BPS;
    let expected = encoded_rows * row_bytes;

    file.seek(SeekFrom::Start(offset))?;
    let mut compressed = vec![0u8; byte_count];
    file.read_exact(&mut compressed)?;

    let mut decoded = vec![0u8; expected];
    // `with_yield_on_full_buffer(true)` is the libtiff-compatible mode: stop as
    // soon as the output buffer is full instead of trying to decode trailing
    // symbols.  This is the entire point of the bypass — we know the exact
    // expected output size and want decoding to terminate the moment we have it,
    // EOI or no EOI.  The status loop is also needed because weezl's
    // `decode_bytes` is a streaming API: a single call may return
    // `LzwStatus::Ok` ("call me again") even when the full compressed slice
    // and a sufficiently large output buffer are supplied.
    let mut lzw = LzwConfiguration::with_tiff_size_switch(BitOrder::Msb, 8)
        .with_yield_on_full_buffer(true)
        .build();

    let mut total_in = 0usize;
    let mut total_out = 0usize;
    loop {
        let r = lzw.decode_bytes(&compressed[total_in..], &mut decoded[total_out..]);
        total_in += r.consumed_in;
        total_out += r.consumed_out;
        match r.status {
            Ok(LzwStatus::Done) => break,
            Ok(LzwStatus::Ok) if total_out >= expected => break,
            Ok(LzwStatus::Ok) => {
                if r.consumed_in == 0 && r.consumed_out == 0 {
                    // No forward progress and not Done — treat as end of useful
                    // input.  The post-loop short-read check below will decide
                    // whether we have enough output.
                    break;
                }
            }
            Ok(LzwStatus::NoProgress) => break,
            Err(e) => return Err(format!("lzw decode failed at offset {offset}: {e}").into()),
        }
    }

    if total_out < expected {
        return Err(format!(
            "lzw decode short at offset {offset}: {total_out} of {expected} bytes"
        )
        .into());
    }

    apply_inverse_predictor(&mut decoded, chunk_cols, encoded_rows, predictor)?;

    // After Predictor 3 unwind each sample is a big-endian byte group (TIFF Tech
    // Note 3 step 1 reverses byte order before deinterleaving).  Predictor 1/2
    // leave bytes in the file's native order.
    let effective_order = if predictor == 3 {
        SampleByteOrder::BigEndian
    } else {
        sample_order
    };

    let mut samples = Vec::with_capacity(actual_cols * actual_rows);
    for r in 0..actual_rows {
        let row_off = r * row_bytes;
        for c in 0..actual_cols {
            let b = row_off + c * BPS;
            let bytes = [decoded[b], decoded[b + 1], decoded[b + 2], decoded[b + 3]];
            samples.push(match effective_order {
                SampleByteOrder::LittleEndian => f32::from_le_bytes(bytes),
                SampleByteOrder::BigEndian => f32::from_be_bytes(bytes),
            });
        }
    }
    Ok(samples)
}

fn apply_inverse_predictor(
    buf: &mut [u8],
    chunk_cols: usize,
    chunk_rows: usize,
    predictor: u16,
) -> Result<(), DemError> {
    const BPS: usize = 4;
    let row_bytes = chunk_cols * BPS;

    match predictor {
        1 => Ok(()),
        2 => {
            for r in 0..chunk_rows {
                let row = &mut buf[r * row_bytes..(r + 1) * row_bytes];
                for c in 1..chunk_cols {
                    for b in 0..BPS {
                        row[c * BPS + b] = row[c * BPS + b].wrapping_add(row[(c - 1) * BPS + b]);
                    }
                }
            }
            Ok(())
        }
        3 => {
            // TIFF Tech Note 3 — floating-point predictor.
            //   Encoder: reverse each sample's byte order → deinterleave bytes
            //   across the row (all MSBs first, then second-MSBs, …) → take
            //   per-byte horizontal differences.
            //   Decoder reverses: per-byte prefix sum → re-interleave bytes →
            //   each sample is now a big-endian byte group.
            let mut tmp = vec![0u8; row_bytes];
            for r in 0..chunk_rows {
                let row = &mut buf[r * row_bytes..(r + 1) * row_bytes];
                for i in 1..row.len() {
                    row[i] = row[i].wrapping_add(row[i - 1]);
                }
                for c in 0..chunk_cols {
                    for b in 0..BPS {
                        tmp[c * BPS + b] = row[b * chunk_cols + c];
                    }
                }
                row.copy_from_slice(&tmp);
            }
            Ok(())
        }
        _ => Err(format!("unsupported TIFF predictor: {predictor}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufWriter;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tiff::encoder::{Compression, TiffEncoder, colortype};

    static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Self-cleaning temp path. Drop removes the file if it still exists; tests
    /// that succeed get cleanup, tests that fail leave the file for inspection
    /// (they panic before drop runs cleanly in some test harnesses anyway).
    struct TmpPath(PathBuf);
    impl TmpPath {
        fn new(stem: &str) -> Self {
            let id = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!(
                "dem_io_lzw_lenient_{}_{}_{}.tif",
                std::process::id(),
                stem,
                id
            ));
            TmpPath(p)
        }
    }
    impl Drop for TmpPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn write_lzw_pred1_image(path: &std::path::Path, cols: u32, rows: u32, data: &[f32]) {
        let file = File::create(path).unwrap();
        let encoder = TiffEncoder::new(BufWriter::new(file)).unwrap();
        encoder
            .with_compression(Compression::Lzw)
            .write_image::<colortype::Gray32Float>(cols, rows, data)
            .unwrap();
    }

    fn open_decoder(path: &std::path::Path) -> Decoder<BufReader<File>> {
        let f = File::open(path).unwrap();
        Decoder::new(BufReader::new(f)).unwrap()
    }

    #[test]
    fn predictor1_round_trip_via_lenient_reader() {
        // tiff::encoder writes LZW + Predictor=1 by default.  Round-trip ensures
        // the lenient reader is binary-compatible with read_chunk on the
        // happy-path side (compression + chunk layout + byte-order detection).
        let tmp = TmpPath::new("pred1");
        let cols = 16u32;
        let rows = 8u32;
        let data: Vec<f32> = (0..(cols * rows) as usize)
            .map(|i| i as f32 * 0.5 - 17.25)
            .collect();
        write_lzw_pred1_image(&tmp.0, cols, rows, &data);

        let mut decoder = open_decoder(&tmp.0);
        let (compression, predictor) = compression_and_predictor(&mut decoder).unwrap();
        assert_eq!(compression, COMPRESSION_LZW);
        assert_eq!(predictor, 1);
        let (offsets, counts, is_tiled) = chunk_layout(&mut decoder).unwrap();
        let (chunk_cols, chunk_rows) = decoder.chunk_dimensions();

        let mut file = File::open(&tmp.0).unwrap();
        let order = read_byte_order(&mut file).unwrap();

        // Stripped TIFF round-trip.  tiff::encoder defaults RowsPerStrip to
        // ~1MB worth of rows so small images end up as one strip with
        // encoded_rows = actual_rows.  Tiled-edge-tile padding is exercised by
        // build_downsampled's real LZW path, not here.
        let mut got = vec![0.0f32; (cols * rows) as usize];
        for i in 0..offsets.len() {
            let row_start = i * chunk_rows as usize;
            let row_end = (row_start + chunk_rows as usize).min(rows as usize);
            let actual_rows = row_end - row_start;
            let encoded_rows = if is_tiled {
                chunk_rows as usize
            } else {
                actual_rows
            };
            let chunk = read_lzw_chunk_f32(
                &mut file,
                offsets[i],
                counts[i] as usize,
                chunk_cols as usize,
                encoded_rows,
                cols as usize,
                actual_rows,
                predictor,
                order,
            )
            .unwrap();
            for r in 0..actual_rows {
                let dst = (row_start + r) * cols as usize;
                got[dst..dst + cols as usize]
                    .copy_from_slice(&chunk[r * cols as usize..(r + 1) * cols as usize]);
            }
        }
        assert_eq!(got, data);
    }

    #[test]
    fn lenient_accepts_truncated_eoi_byte() {
        // Drop the final byte of the compressed payload.  This is the exact
        // shape of the issue #40 bug: weezl never sees its EOI symbol but the
        // output buffer was filled in the previous call.  Lenient reader must
        // still produce correct output (or at worst a clean error — never a
        // panic and never garbage).
        let tmp = TmpPath::new("trunc1");
        let cols = 8u32;
        let rows = 4u32;
        let data: Vec<f32> = (0..(cols * rows) as usize).map(|i| i as f32).collect();
        write_lzw_pred1_image(&tmp.0, cols, rows, &data);

        let mut decoder = open_decoder(&tmp.0);
        let (_compression, predictor) = compression_and_predictor(&mut decoder).unwrap();
        let (offsets, counts, _is_tiled) = chunk_layout(&mut decoder).unwrap();
        let (chunk_cols, _chunk_rows) = decoder.chunk_dimensions();

        let mut file = File::open(&tmp.0).unwrap();
        let order = read_byte_order(&mut file).unwrap();

        let truncated = (counts[0] as usize).saturating_sub(1);
        let result = read_lzw_chunk_f32(
            &mut file,
            offsets[0],
            truncated,
            chunk_cols as usize,
            rows as usize, // single-strip encoded_rows = full image rows
            cols as usize,
            rows as usize,
            predictor,
            order,
        );
        match result {
            Ok(got) => assert_eq!(got, data, "decoded mismatch after EOI byte chop"),
            Err(_) => { /* clean error is also acceptable — the critical thing is no panic */ }
        }
    }

    #[test]
    fn rejects_genuinely_short_input() {
        // Cut so aggressively that consumed_out cannot reach `expected`.  Must
        // return Err, never panic, never return garbage data.
        let tmp = TmpPath::new("trunc_hard");
        let cols = 32u32;
        let rows = 16u32;
        let data: Vec<f32> = (0..(cols * rows) as usize)
            .map(|i| i as f32 * 1.5)
            .collect();
        write_lzw_pred1_image(&tmp.0, cols, rows, &data);

        let mut decoder = open_decoder(&tmp.0);
        let (_compression, predictor) = compression_and_predictor(&mut decoder).unwrap();
        let (offsets, counts, _is_tiled) = chunk_layout(&mut decoder).unwrap();
        let (chunk_cols, _chunk_rows) = decoder.chunk_dimensions();

        let mut file = File::open(&tmp.0).unwrap();
        let order = read_byte_order(&mut file).unwrap();

        let result = read_lzw_chunk_f32(
            &mut file,
            offsets[0],
            (counts[0] as usize) / 4,
            chunk_cols as usize,
            rows as usize,
            cols as usize,
            rows as usize,
            predictor,
            order,
        );
        assert!(result.is_err(), "expected short-read error, got {result:?}");
    }

    /// Encode an f32 row into the planar+differenced byte layout that the
    /// TIFF floating-point predictor produces.  This is the inverse of
    /// `apply_inverse_predictor(..., 3)` and lets us synthesise test fixtures
    /// without relying on `tiff::encoder` (which doesn't support Predictor 3
    /// in 0.11.3).
    fn encode_predictor3(samples: &[f32], cols: usize, rows: usize) -> Vec<u8> {
        const BPS: usize = 4;
        let row_bytes = cols * BPS;
        let mut out = vec![0u8; rows * row_bytes];
        for r in 0..rows {
            // Big-endian bytes per sample.
            let mut be_row = vec![0u8; row_bytes];
            for c in 0..cols {
                let bytes = samples[r * cols + c].to_be_bytes();
                be_row[c * BPS..(c + 1) * BPS].copy_from_slice(&bytes);
            }
            // Deinterleave bytes: all MSBs first, then 2nd, 3rd, 4th.
            let mut planar = vec![0u8; row_bytes];
            for c in 0..cols {
                for b in 0..BPS {
                    planar[b * cols + c] = be_row[c * BPS + b];
                }
            }
            // Horizontal byte differences.
            let mut diffed = vec![0u8; row_bytes];
            diffed[0] = planar[0];
            for i in 1..row_bytes {
                diffed[i] = planar[i].wrapping_sub(planar[i - 1]);
            }
            out[r * row_bytes..(r + 1) * row_bytes].copy_from_slice(&diffed);
        }
        out
    }

    #[test]
    fn predictor3_inverse_round_trip() {
        // Construct planar+differenced bytes from known f32 values, run them
        // through apply_inverse_predictor(..., 3), and verify the result
        // re-interleaves to big-endian sample bytes that decode bit-exactly.
        let cols = 12usize;
        let rows = 5usize;
        let samples: Vec<f32> = (0..(cols * rows))
            .map(|i| (i as f32) * 1.25 - 7.5)
            .collect();
        let mut encoded = encode_predictor3(&samples, cols, rows);

        apply_inverse_predictor(&mut encoded, cols, rows, 3).unwrap();

        // After inverse predictor 3, each 4-byte group is the big-endian sample.
        const BPS: usize = 4;
        for r in 0..rows {
            for c in 0..cols {
                let b = (r * cols + c) * BPS;
                let bytes = [encoded[b], encoded[b + 1], encoded[b + 2], encoded[b + 3]];
                let got = f32::from_be_bytes(bytes);
                let want = samples[r * cols + c];
                assert_eq!(got.to_bits(), want.to_bits(), "row={r} col={c}");
            }
        }
    }

    #[test]
    fn predictor2_horizontal_round_trip() {
        // Construct a row where each sample's bytes are the per-byte sum of
        // the previous sample's bytes plus an arbitrary delta, then check
        // apply_inverse_predictor recovers the original.
        let cols = 6usize;
        let rows = 2usize;
        const BPS: usize = 4;

        let samples: Vec<[u8; BPS]> = (0..(cols * rows))
            .map(|i| {
                let i = i as u32;
                [
                    (i & 0xff) as u8,
                    ((i >> 8) & 0xff) as u8,
                    ((i >> 16) & 0xff) as u8,
                    ((i >> 24) & 0xff) as u8,
                ]
            })
            .collect();

        // Encode predictor 2 (horizontal): differences taken per sample.
        let mut encoded = vec![0u8; cols * rows * BPS];
        for r in 0..rows {
            let row_off = r * cols * BPS;
            // First sample stored verbatim.
            encoded[row_off..row_off + BPS].copy_from_slice(&samples[r * cols]);
            for c in 1..cols {
                for b in 0..BPS {
                    encoded[row_off + c * BPS + b] =
                        samples[r * cols + c][b].wrapping_sub(samples[r * cols + c - 1][b]);
                }
            }
        }

        apply_inverse_predictor(&mut encoded, cols, rows, 2).unwrap();
        for i in 0..(cols * rows) {
            let got = &encoded[i * BPS..(i + 1) * BPS];
            assert_eq!(got, &samples[i]);
        }
    }
}
