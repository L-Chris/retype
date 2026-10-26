//! Original encoded image bytes and bounded Windows DIB conversion. No clipboard or disk access.
use crate::clipboard_protocol::{MAX_IMAGE, MAX_PIXELS};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::io::Cursor;

pub const MIME_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/bmp",
    "image/tiff",
];
#[derive(Debug, serde::Serialize)]
pub struct Info {
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}
pub fn mime_type(bytes: &[u8]) -> Option<&'static str> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::WebP => Some("image/webp"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::Bmp => Some("image/bmp"),
        image::ImageFormat::Tiff => Some("image/tiff"),
        _ => None,
    }
}
fn reader(bytes: &[u8]) -> Option<image::ImageReader<Cursor<&[u8]>>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PIXELS as u32);
    limits.max_image_height = Some(MAX_PIXELS as u32);
    limits.max_alloc = Some(MAX_PIXELS * 8);
    reader.limits(limits);
    Some(reader)
}
pub fn information(bytes: &[u8]) -> Option<Info> {
    if bytes.len() > MAX_IMAGE {
        return None;
    }
    let mime = mime_type(bytes)?;
    let (width, height) = reader(bytes)?.into_dimensions().ok()?;
    (width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS).then_some(
        Info {
            mime,
            width,
            height,
        },
    )
}
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    information(bytes).map(|info| (info.width, info.height))
}
/// Missing format negotiation means the earlier PNG-only image extension.
pub fn peer_formats(message: &serde_json::Value) -> Vec<String> {
    if message["images"] != true {
        return Vec::new();
    }
    match message["imageFormats"].as_array() {
        Some(formats) => MIME_TYPES
            .iter()
            .filter(|mime| formats.iter().any(|v| v.as_str() == Some(**mime)))
            .map(|s| (*s).to_owned())
            .collect(),
        None => vec!["image/png".into()],
    }
}

pub fn encode(bytes: &[u8]) -> Option<String> {
    dimensions(bytes)?;
    Some(STANDARD.encode(bytes))
}
pub fn decode(encoded: &str) -> Option<Vec<u8>> {
    if encoded.len() > MAX_IMAGE.div_ceil(3) * 4 {
        return None;
    }
    let bytes = STANDARD.decode(encoded).ok()?;
    dimensions(&bytes)?;
    Some(bytes)
}
fn rgba_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().ok()?.write_image_data(rgba).ok()?;
    }
    (out.len() <= MAX_IMAGE).then_some(out)
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
/// Packed 24/32-bit BI_RGB or standard BGRA BI_BITFIELDS DIB, top-down or bottom-up.
pub fn from_dib(dib: &[u8]) -> Option<Vec<u8>> {
    let header = u32_at(dib, 0)? as usize;
    if ![40, 108, 124].contains(&header) || dib.len() < header {
        return None;
    }
    let width = u32_at(dib, 4)? as i32;
    let signed_height = u32_at(dib, 8)? as i32;
    let height = signed_height.checked_abs()? as usize;
    if width <= 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
        return None;
    }
    let width = width as usize;
    if dib.get(12..14)? != [1, 0] {
        return None;
    }
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    if ![24, 32].contains(&bits) || u32_at(dib, 32)? != 0 {
        return None;
    }
    let compression = u32_at(dib, 16)?;
    let mut offset = header;
    let mut alpha = false;
    if compression == 3 && bits == 32 {
        let masks = if header == 40 {
            offset += 12;
            40
        } else {
            40
        };
        if [
            u32_at(dib, masks)?,
            u32_at(dib, masks + 4)?,
            u32_at(dib, masks + 8)?,
        ] != [0xff0000, 0xff00, 0xff]
        {
            return None;
        }
        alpha = header >= 108 && u32_at(dib, 52)? == 0xff000000;
    } else if compression != 0 {
        return None;
    }
    let pixel_bytes = bits as usize / 8;
    let stride = (width * pixel_bytes).div_ceil(4) * 4;
    let pixels = dib.get(offset..offset.checked_add(stride.checked_mul(height)?)?)?;
    let mut rgba = vec![0; width * height * 4];
    for y in 0..height {
        let source_y = if signed_height < 0 { y } else { height - 1 - y };
        for x in 0..width {
            let s = source_y * stride + x * pixel_bytes;
            let d = (y * width + x) * 4;
            rgba[d..d + 4].copy_from_slice(&[
                pixels[s + 2],
                pixels[s + 1],
                pixels[s],
                if alpha { pixels[s + 3] } else { 255 },
            ]);
        }
    }
    rgba_png(width as u32, height as u32, &rgba)
}
/// Add a bitmap representation for Windows apps while retaining the original encoded format.
/// Animated sources remain intact in the original format; this fallback uses their first frame.
pub fn to_dib(bytes: &[u8]) -> Option<Vec<u8>> {
    let (width, height) = dimensions(bytes)?;
    let rgba = reader(bytes)?.decode().ok()?.to_rgba8();
    let pixels = rgba.as_raw();
    let mut dib = vec![0u8; 124 + width as usize * height as usize * 4];
    for (offset, value) in [
        (0, 124),
        (4, width),
        (8, height),
        (16, 3),
        (20, width * height * 4),
        (40, 0xff0000),
        (44, 0xff00),
        (48, 0xff),
        (52, 0xff000000),
        (56, 0x73524742),
    ] {
        dib[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    for y in 0..height as usize {
        for x in 0..width as usize {
            let s = (y * width as usize + x) * 4;
            let (r, g, b, a) = (pixels[s], pixels[s + 1], pixels[s + 2], pixels[s + 3]);
            let d = 124 + ((height as usize - 1 - y) * width as usize + x) * 4;
            dib[d..d + 4].copy_from_slice(&[b, g, r, a]);
        }
    }
    Some(dib)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_formats_keep_exact_bytes_and_have_a_windows_bitmap_fallback() -> Result<(), String> {
        for (format, mime) in [
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::Jpeg, "image/jpeg"),
            (image::ImageFormat::WebP, "image/webp"),
            (image::ImageFormat::Gif, "image/gif"),
            (image::ImageFormat::Bmp, "image/bmp"),
            (image::ImageFormat::Tiff, "image/tiff"),
        ] {
            let rgb =
                image::RgbImage::from_raw(2, 1, vec![255, 0, 0, 0, 0, 255]).ok_or("pixels")?;
            let mut out = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(rgb)
                .write_to(&mut out, format)
                .map_err(|e| e.to_string())?;
            let bytes = out.into_inner();
            assert_eq!(mime_type(&bytes), Some(mime));
            assert_eq!(dimensions(&bytes), Some((2, 1)));
            assert_eq!(
                decode(&encode(&bytes).ok_or("encode")?),
                Some(bytes.clone())
            );
            let dib = to_dib(&bytes).ok_or(format!("DIB fallback for {mime}"))?;
            assert_eq!(dimensions(&from_dib(&dib).ok_or("DIB")?), Some((2, 1)));
        }
        Ok(())
    }
    #[test]
    fn peers_negotiate_common_formats_without_sending_jpeg_to_png_only_builds() -> Result<(), String>
    {
        use crate::clipboard_protocol::Journal;
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 1))
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .map_err(|e| e.to_string())?;
        let mut journal = Journal::default();
        let clip = journal
            .local_image(&"a".repeat(32), 1, jpeg.get_ref(), 1_000)
            .ok_or("JPEG event")?;
        assert!(!clip.supported_by(&peer_formats(&serde_json::json!({"images":true}))));
        assert!(!clip.supported_by(&peer_formats(&serde_json::json!({}))));
        assert!(clip.supported_by(&peer_formats(
            &serde_json::json!({"images":true,"imageFormats":MIME_TYPES})
        )));
        Ok(())
    }
    #[test]
    fn transparent_png_round_trips_through_windows_dib() -> Result<(), String> {
        let original = rgba_png(
            2,
            2,
            &[255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 1, 2, 3, 255],
        )
        .ok_or("PNG")?;
        let dib = to_dib(&original).ok_or("DIB")?;
        let roundtrip = from_dib(&dib).ok_or("PNG roundtrip")?;
        assert_eq!(to_dib(&roundtrip), Some(dib));
        assert_eq!(decode(&encode(&original).ok_or("encode")?), Some(original));
        Ok(())
    }
    #[test]
    fn malformed_oversized_and_truncated_images_are_rejected() {
        assert!(decode("not base64").is_none());
        assert!(encode(b"not PNG").is_none());
        assert!(from_dib(&[0; 124]).is_none());
        let mut dib = vec![0; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        dib[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(from_dib(&dib).is_none());
    }
    #[test]
    fn padded_top_down_24_bit_dib_is_supported() -> Result<(), String> {
        let mut dib = vec![0; 48];
        for (offset, value) in [(0, 40u32), (4, 1), (8, (-2i32) as u32)] {
            dib[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        dib[12] = 1;
        dib[14] = 24;
        dib[40..48].copy_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        let png = from_dib(&dib).ok_or("DIB")?;
        let v5 = to_dib(&png).ok_or("PNG")?;
        assert_eq!(&v5[124..], &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(from_dib(&dib[..47]).is_none());
        Ok(())
    }
}
