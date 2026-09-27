//! Intrinsic image-dimension sniffing from header bytes.
//!
//! The server stores the dimensions alongside each image in the image
//! store and emits `width`/`height` attributes on chapter `<img>` tags,
//! so browsers can reserve layout space (aspect-ratio) before the bytes
//! arrive — without it, every image load grows the page and the reader
//! view keeps re-anchoring while scrolling.
//!
//! Only container headers are parsed (no full decode); unsupported or
//! malformed data yields `None` and the attribute is simply omitted.

/// Extract `(width, height)` from the leading bytes of an encoded image
/// (PNG, GIF, JPEG or WebP).
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png_dimensions(bytes);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        // Logical screen descriptor directly after the 6-byte signature.
        let b = bytes.get(6..12)?;
        return Some((u16le(&b[0..2]) as u32, u16le(&b[2..4]) as u32));
    }
    if bytes.starts_with(b"\xff\xd8") {
        return jpeg_dimensions(bytes);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return webp_dimensions(bytes);
    }
    None
}

fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn u16be(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// PNG: first chunk must be IHDR — length(4) "IHDR" width(4) height(4),
/// all big-endian.
fn png_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(b[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(b[20..24].try_into().ok()?);
    Some((w, h))
}

/// JPEG: walk the segment chain for the SOF0–SOF15 marker (skipping DHT,
/// DQT and other payload-carrying segments); height/width are big-endian.
fn jpeg_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 4 <= b.len() {
        if b[i] != 0xff {
            return None; // desynchronized
        }
        let marker = b[i + 1];
        // Standalone markers without a length field.
        if !(0xd0..=0xd9).contains(&marker) && marker != 0x01 && marker != 0xff {
            let len = usize::from(u16be(&b[i + 2..i + 4]));
            // SOF0..SOF15 except DHT (0xc4), JPG (0xc8), DAC (0xcc).
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                if i + 9 > b.len() {
                    return None;
                }
                let h = u16be(&b[i + 5..i + 7]) as u32;
                let w = u16be(&b[i + 7..i + 9]) as u32;
                return Some((w, h));
            }
            i += 2 + len;
        } else {
            i += 2;
        }
    }
    None
}

/// WebP: VP8 (lossy) / VP8L (lossless) / VP8X (extended) sub-chunks.
fn webp_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let chunk = b.get(12..16)?;
    match chunk {
        b"VP8 " => {
            // Chunk header (4) + frame tag (3) + sync code (3), then
            // little-endian 16-bit dims (14 significant bits each).
            let w = u16le(b.get(26..28)?) as u32 & 0x3fff;
            let h = u16le(b.get(28..30)?) as u32 & 0x3fff;
            Some((w, h))
        }
        b"VP8L" => {
            // Chunk header (4) + signature byte (1), then 14/14/15/15-bit
            // packed fields: width-1, height-1.
            let d = b.get(21..25)?;
            let bits = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
            let w = (bits & 0x3fff) + 1;
            let h = ((bits >> 14) & 0x3fff) + 1;
            Some((w, h))
        }
        b"VP8X" => {
            // 24-bit canvas size minus one, little-endian.
            let d = b.get(24..30)?;
            let w = u32::from(d[0]) | u32::from(d[1]) << 8 | u32::from(d[2]) << 16;
            let h = u32::from(d[3]) | u32::from(d[4]) << 8 | u32::from(d[5]) << 16;
            Some((w + 1, h + 1))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but structurally valid PNG header (IHDR only).
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v
    }

    /// Minimal JPEG with an SOF0 segment after a fake DHT segment.
    fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0xff, 0xd8];
        // DHT segment (must be skipped by the walker).
        v.extend_from_slice(&[0xff, 0xc4, 0x00, 0x04, 0xaa, 0xbb]);
        // SOF0: len(2), precision(1), height(2), width(2).
        v.extend_from_slice(&[0xff, 0xc0, 0x00, 0x05, 0x08]);
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v
    }

    #[test]
    fn png_gif_jpeg_webp() {
        assert_eq!(image_dimensions(&png(800, 600)), Some((800, 600)));
        assert_eq!(image_dimensions(&png(1, 1)), Some((1, 1)));

        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&100u16.to_le_bytes());
        gif.extend_from_slice(&50u16.to_le_bytes());
        gif.extend_from_slice(&[0xf0, 0, 0]);
        assert_eq!(image_dimensions(&gif), Some((100, 50)));

        assert_eq!(image_dimensions(&jpeg(640, 480)), Some((640, 480)));

        // VP8 lossy: chunk size, 3-byte frame tag, 3-byte sync code, dims.
        let mut vp8 = b"RIFF\x1e\x00\x00\x00WEBPVP8 \x0a\x00\x00\x00".to_vec();
        vp8.extend_from_slice(&[0x30, 0x01, 0x00]); // frame tag
        vp8.extend_from_slice(&[0x9d, 0x01, 0x2a]); // sync code
        vp8.extend_from_slice(&320u16.to_le_bytes()); // width & 0x3fff
        vp8.extend_from_slice(&200u16.to_le_bytes());
        vp8.extend_from_slice(&[0, 0, 0, 0]); // trailing payload
        assert_eq!(image_dimensions(&vp8), Some((320, 200)));

        // VP8L lossless: chunk size + signature byte, then packed bits.
        let mut vp8l = b"RIFF\x14\x00\x00\x00WEBPVP8L\x06\x00\x00\x00".to_vec();
        vp8l.push(0x00); // signature
        let bits: u32 = (319) | (199 << 14);
        vp8l.extend_from_slice(&bits.to_le_bytes());
        vp8l.extend_from_slice(&[0, 0]); // trailing payload
        assert_eq!(image_dimensions(&vp8l), Some((320, 200)));

        // VP8X extended: chunk size + flags/reserved + 24-bit canvas-minus-one.
        let mut vp8x = b"RIFF\x0f\x00\x00\x00WEBPVP8X\x0a\x00\x00\x00".to_vec();
        vp8x.extend_from_slice(&[0, 0, 0, 0]); // flags + reserved
        vp8x.extend_from_slice(&[0xff, 0x01, 0x00]); // width-1 = 511
        vp8x.extend_from_slice(&[0xff, 0x00, 0x00]); // height-1 = 255
        assert_eq!(image_dimensions(&vp8x), Some((512, 256)));

        // Unsupported / truncated input.
        assert_eq!(image_dimensions(b"<html>"), None);
        assert_eq!(image_dimensions(&[]), None);
        assert_eq!(image_dimensions(b"\x89PNG\r\n\x1a\nshort"), None);
    }
}
