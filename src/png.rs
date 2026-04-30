//! Minimal PNG decoder — handles 8-bit RGB/RGBA images (color type 2 or 6).
//! Uses `flate2` for DEFLATE decompression (already a dependency).
//! Only supports filter methods 0-4 (no Adam7 interlace, no palette images).

use flate2::read::ZlibDecoder;
use std::io::Read;

pub fn decode_png(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if data.len() < 8 || &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut pos: usize = 8;
    let mut width: u32 = 0;
    let mut height: u32 = 0;
    let mut idat: Vec<u8> = Vec::new();
    let mut bpp: usize = 0;

    while pos + 12 <= data.len() {
        let chunk_len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let chunk_type = &data[pos + 4..pos + 8];
        pos += 8;
        if pos + chunk_len > data.len() {
            return None;
        }
        match chunk_type {
            b"IHDR" if chunk_len >= 13 => {
                width =
                    u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
                height = u32::from_be_bytes([
                    data[pos + 4],
                    data[pos + 5],
                    data[pos + 6],
                    data[pos + 7],
                ]);
                let bit_depth = data[pos + 8];
                let color_type = data[pos + 9];
                if bit_depth != 8 {
                    return None;
                }
                bpp = match color_type {
                    2 => 3, // RGB
                    6 => 4, // RGBA
                    _ => return None,
                };
            }
            b"IDAT" => {
                idat.extend_from_slice(&data[pos..pos + chunk_len]);
            }
            b"IEND" => break,
            _ => {}
        }
        pos += chunk_len + 4; // skip data + CRC
    }

    if width == 0 || height == 0 || idat.is_empty() || bpp == 0 {
        return None;
    }

    let mut decoder = ZlibDecoder::new(&idat[..]);
    let mut raw: Vec<u8> = Vec::new();
    decoder.read_to_end(&mut raw).ok()?;

    let row_stride = width as usize * bpp;
    let mut out: Vec<u8> = vec![0u8; width as usize * height as usize * 4];
    let mut row_buf = vec![0u8; row_stride];
    let mut prev_row = vec![0u8; row_stride];
    let mut raw_offset: usize = 0;

    for y in 0..height as usize {
        if raw_offset >= raw.len() {
            break;
        }
        let filter = raw[raw_offset];
        raw_offset += 1;

        let _scanline_len = 1 + row_stride; // raw: filter byte + pixel data
        let row_data = &raw[raw_offset.saturating_sub(1)..raw_offset + row_stride];
        raw_offset += row_stride;

        for x in 0..row_stride {
            let a = if x >= bpp { row_buf[x - bpp] } else { 0 };
            let b = prev_row[x];
            let c = if x >= bpp { prev_row[x - bpp] } else { 0 };
            let raw_val = row_data[x + 1]; // +1 to skip the filter byte

            row_buf[x] = match filter {
                0 => raw_val,
                1 => raw_val.wrapping_add(a),
                2 => raw_val.wrapping_add(b),
                3 => raw_val.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => raw_val.wrapping_add(paeth(a, b, c)),
                _ => return None,
            };
        }

        let out_row = y * width as usize * 4;
        for x in 0..width as usize {
            for ch in 0..bpp {
                out[out_row + x * 4 + ch] = row_buf[x * bpp + ch];
            }
            if bpp == 3 {
                out[out_row + x * 4 + 3] = 255; // no alpha channel -> opaque
            }
        }

        std::mem::swap(&mut row_buf, &mut prev_row);
    }

    Some((out, width, height))
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}
