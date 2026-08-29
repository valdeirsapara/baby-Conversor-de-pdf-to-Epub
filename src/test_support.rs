//! Builders shared by the unit tests. Everything here is `#[cfg(test)]`-only: the crate
//! is a binary, so tests live in `mod tests` inside each module and pull their fixtures
//! from this one place instead of re-declaring structs over and over.

use crate::cover::Cover;
use crate::pdf_model::{ExtractedDoc, FlowEvent, ImageAsset, PageFlow, TextRun};

/// An ordinary body line, starting flush at the column margin.
pub fn line(text: &str, y: f64, font_size: f64) -> FlowEvent {
    line_at(text, y, font_size, BODY_LEFT)
}

/// Left edge of the text column used by the fixtures, as a page fraction.
pub const BODY_LEFT: f64 = 0.12;

/// A line starting at an explicit x, for exercising indent and right-alignment detection.
pub fn line_at(text: &str, y: f64, font_size: f64, x_start: f64) -> FlowEvent {
    FlowEvent::Line(TextRun { text: text.to_string(), y, font_size, x_start })
}

/// A page whose events are laid out top-to-bottom in the order given, which is the
/// invariant `pdf_parse` guarantees to the rest of the pipeline.
pub fn page(page_num: u32, events: Vec<FlowEvent>) -> PageFlow {
    let events = events
        .into_iter()
        .enumerate()
        .map(|(i, e)| (i as f64 * 10.0, e))
        .collect();
    PageFlow { page_num, events }
}

/// Like `page`, but keying each event by the y the line itself carries — the faithful
/// shape for anything that reasons about vertical spacing.
pub fn page_at(page_num: u32, events: Vec<FlowEvent>) -> PageFlow {
    let events = events
        .into_iter()
        .map(|e| {
            let y = match &e {
                FlowEvent::Line(run) => run.y,
                FlowEvent::Image(_) => 0.0,
            };
            (y, e)
        })
        .collect();
    PageFlow { page_num, events }
}

pub fn doc(pages: Vec<PageFlow>, images: Vec<ImageAsset>) -> ExtractedDoc {
    ExtractedDoc { pages, images }
}

/// A JPEG-flavoured image asset. The bytes are opaque to everything under test here —
/// only the dimensions and the filename matter — so they stay deliberately tiny.
pub fn jpeg(filename: &str, width: u32, height: u32) -> ImageAsset {
    ImageAsset {
        filename: filename.to_string(),
        bytes: vec![0xFF, 0xD8, 0xFF, 0xD9],
        mime: "image/jpeg",
        width,
        height,
    }
}

pub fn png(filename: &str, width: u32, height: u32) -> ImageAsset {
    ImageAsset {
        filename: filename.to_string(),
        bytes: vec![0x89, b'P', b'N', b'G'],
        mime: "image/png",
        width,
        height,
    }
}

/// A cover that came from nowhere in particular — for tests that need *a* cover but
/// aren't exercising cover selection itself.
pub fn some_cover() -> Cover {
    Cover {
        filename: "cover.jpg".to_string(),
        mime: "image/jpeg",
        bytes: vec![0xFF, 0xD8, 0xFF, 0xD9],
        width: 800,
        height: 1280,
        consumed_image: None,
    }
}

/// Encodes a real, decodable image so tests can round-trip it through `image`.
pub fn real_png_bytes(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(width, height, image::Rgb([10, 20, 30]));
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .expect("PNG de teste");
    buf
}
