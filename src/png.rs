//! The indexed PNG encoder `raster/png-indexed;v=1` names.
//!
//! Not a post-processing step and not a convenience. `tiny_skia`'s own `save_png` writes
//! RGBA8, which is fourteen times the bytes for identical pixels and is not the thing the
//! form identifier names - a worker handed one would have to quantise before it could send,
//! which is exactly the decision the form exists to have already made. The thresholding that
//! produces the palette indices happens here anyway, so the encoder belongs here too.

use std::io::Write;

/// Colour type 3 (indexed) at bit depth 2: four palette entries, two bits per pixel.
const COLOR_TYPE_INDEXED: u8 = 3;
const BIT_DEPTH: u8 = 2;

pub fn encode(width: u32, height: u32, indices: &[u8], palette: &[[u8; 3]]) -> Result<Vec<u8>, String> {
    if palette.is_empty() || palette.len() > 4 {
        return Err(format!("a bit depth of {BIT_DEPTH} carries 1 to 4 palette entries, not {}", palette.len()));
    }
    if indices.len() != (width as usize) * (height as usize) {
        return Err("index buffer does not match the declared dimensions".into());
    }

    let mut out = Vec::with_capacity(indices.len() / 4 + 1024);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(BIT_DEPTH);
    ihdr.push(COLOR_TYPE_INDEXED);
    ihdr.extend_from_slice(&[0, 0, 0]); // deflate, adaptive filtering, no interlace
    chunk(&mut out, b"IHDR", &ihdr);

    let mut plte = Vec::with_capacity(palette.len() * 3);
    for entry in palette {
        plte.extend_from_slice(entry);
    }
    chunk(&mut out, b"PLTE", &plte);

    // Filter type 0 on every scanline. Indexed data at two bits per pixel gains nothing from
    // the predictors - they work on bytes, and a byte here holds four unrelated palette
    // indices - so choosing between them would cost a pass and save nothing.
    let per_byte = 8 / BIT_DEPTH as usize;
    let stride = (width as usize).div_ceil(per_byte);
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        let row = &indices[y * width as usize..(y + 1) * width as usize];
        let mut byte = 0u8;
        let mut filled = 0usize;
        for index in row {
            byte = (byte << BIT_DEPTH) | (index & 0b11);
            filled += 1;
            if filled == per_byte {
                raw.push(byte);
                byte = 0;
                filled = 0;
            }
        }
        if filled > 0 {
            raw.push(byte << (BIT_DEPTH as usize * (per_byte - filled)));
        }
    }

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&raw).map_err(|e| e.to_string())?;
    let idat = encoder.finish().map_err(|e| e.to_string())?;
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);

    Ok(out)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(kind);
    hasher.update(data);
    out.extend_from_slice(&hasher.finalize().to_be_bytes());
}

/// Parses `#rrggbb` as the profile writes it.
pub fn parse_colour(value: &str) -> Result<[u8; 3], String> {
    let hex = value.strip_prefix('#').ok_or_else(|| format!("{value} is not a #rrggbb colour"))?;
    if hex.len() != 6 {
        return Err(format!("{value} is not a #rrggbb colour"));
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string());
    Ok([byte(0)?, byte(2)?, byte(4)?])
}
