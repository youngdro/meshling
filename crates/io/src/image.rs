//! PNG encoding of premultiplied pixels, and texture content hashes.

use std::io::Cursor;

use meshling_geom::Size;
use meshling_plan::Pixels;

use crate::psd::premultiply;

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("PNG decode: {0}")]
    Decode(String),
    #[error("PNG encode: {0}")]
    Encode(String),
}

/// Hex BLAKE3 of premultiplied RGBA8 pixels: the `content_hash` of a `TextureAsset`.
pub fn content_hash(pixels: &Pixels) -> String {
    blake3::hash(&pixels.rgba).to_hex().to_string()
}

/// Premultiplied to straight alpha, rounded to nearest. Exact inverse of [`premultiply`] on
/// every value `premultiply` can produce.
pub fn unpremultiply(c: u8, a: u8) -> u8 {
    if a == 0 { 0 } else { ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8 }
}

/// Encode as an 8-bit straight-alpha RGBA PNG (what image viewers and editors expect).
pub fn encode_png(pixels: &Pixels) -> Result<Vec<u8>, ImageError> {
    let straight: Vec<u8> = pixels
        .rgba
        .chunks(4)
        .flat_map(|p| [unpremultiply(p[0], p[3]), unpremultiply(p[1], p[3]), unpremultiply(p[2], p[3]), p[3]])
        .collect();
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, pixels.size.width, pixels.size.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| ImageError::Encode(e.to_string()))?;
    writer.write_image_data(&straight).map_err(|e| ImageError::Encode(e.to_string()))?;
    writer.finish().map_err(|e| ImageError::Encode(e.to_string()))?;
    Ok(out)
}

/// Decode any 8- or 16-bit PNG into premultiplied RGBA8.
pub fn decode_png(bytes: &[u8]) -> Result<Pixels, ImageError> {
    let err = |e: png::DecodingError| ImageError::Decode(e.to_string());
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(err)?;
    let len = reader.output_buffer_size().ok_or_else(|| ImageError::Decode("image too large".into()))?;
    let mut buf = vec![0; len];
    let info = reader.next_frame(&mut buf).map_err(err)?;
    let data = &buf[..info.buffer_size()];
    let n = info.width as usize * info.height as usize;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => data.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err(ImageError::Decode("unexpanded palette".into())),
    };
    if rgba.len() != n * 4 {
        return Err(ImageError::Decode("unexpected pixel buffer size".into()));
    }
    let premul: Vec<u8> = rgba
        .chunks(4)
        .flat_map(|p| [premultiply(p[0], p[3]), premultiply(p[1], p[3]), premultiply(p[2], p[3]), p[3]])
        .collect();
    Pixels::new(Size::new(info.width, info.height), premul).ok_or_else(|| ImageError::Decode("size".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_values_survive_a_straight_round_trip() {
        for a in 0..=255u8 {
            for c in 0..=a {
                assert_eq!(premultiply(unpremultiply(c, a), a), c, "c={c} a={a}");
            }
        }
    }

    #[test]
    fn png_round_trip_is_lossless_and_hash_is_stable() {
        let rgba: Vec<u8> = (0..64u32)
            .flat_map(|i| {
                let a = (i * 4) as u8;
                [premultiply(200, a), premultiply(i as u8, a), 0, a]
            })
            .collect();
        let pixels = Pixels::new(Size::new(8, 8), rgba).unwrap();
        let back = decode_png(&encode_png(&pixels).unwrap()).unwrap();
        assert_eq!(back, pixels);
        assert_eq!(content_hash(&back), content_hash(&pixels));
        assert_eq!(content_hash(&pixels).len(), 64);
        assert!(decode_png(b"not a png").is_err());
    }
}
