//! Window images for the assistant's screenshot fallback: X11 reads the screen directly; Wayland
//! asks the Screenshot portal (the desktop may confirm with the user) and crops to the window
//! when its position is known. Scaled to at most 1400 pixels, as JPEG.
use super::Session;
use image::GenericImageView;

type Frame = (f64, f64, f64, f64);

/// An image of `frame` (screen pixels), or of the whole screen when it can't be located.
/// Returns (jpeg, width, height, the screen area the image shows).
pub fn capture(frame: Option<Frame>) -> Result<(Vec<u8>, u32, u32, Option<Frame>), String> {
    let (img, area) = match super::session() {
        Session::X11 => {
            let f = frame.ok_or("the window's position is unknown")?;
            (image::DynamicImage::ImageRgb8(super::x11::capture(f)?), Some(f))
        }
        Session::Wayland => {
            let path = super::portal::screenshot()?;
            let r = image::open(&path).map_err(|e| format!("couldn't read the screenshot: {e}"));
            remove_shot(&path);
            let full = r?;
            match frame.and_then(|f| crop_box(f, full.dimensions())) {
                Some((x, y, w, h)) => (full.crop_imm(x, y, w, h), Some((f64::from(x), f64::from(y), f64::from(w), f64::from(h)))),
                None => (full, None),
            }
        }
    };
    let img = if img.width().max(img.height()) > 1400 { img.resize(1400, 1400, image::imageops::FilterType::Triangle) } else { img };
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 75);
    img.to_rgb8().write_with_encoder(enc).map_err(|e| e.to_string())?;
    if out.len() > 4 * 1024 * 1024 {
        return Err("screenshot too large".into());
    }
    Ok((out, w, h, area))
}

/// The part of a screen image a window covers, if it lies inside it.
pub fn crop_box(f: Frame, (iw, ih): (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = f;
    if x < 0.0 || y < 0.0 || w < 20.0 || h < 20.0 || x + w > f64::from(iw) + 1.0 || y + h > f64::from(ih) + 1.0 {
        return None;
    }
    let (x, y) = (x as u32, y as u32);
    Some((x, y, (w as u32).min(iw - x), (h as u32).min(ih - y)))
}

/// The portal saves screenshots where the user keeps theirs: remove ours (only a fresh PNG).
fn remove_shot(p: &std::path::Path) {
    let fresh = std::fs::metadata(p).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 120);
    if fresh && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_are_cropped_only_when_inside_the_screen() {
        assert_eq!(super::crop_box((100.0, 50.0, 800.0, 600.0), (1920, 1080)), Some((100, 50, 800, 600)));
        assert_eq!(super::crop_box((0.0, 0.0, 0.0, 0.0), (1920, 1080)), None, "unknown position (Wayland)");
        assert_eq!(super::crop_box((1500.0, 50.0, 800.0, 600.0), (1920, 1080)), None, "off the image");
    }
}
