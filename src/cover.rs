use std::path::Path;

use ab_glyph::{Font, FontRef, Glyph, PxScale, ScaleFont};
use image::{Rgb, RgbImage};

use crate::epub_gen::escape_html;
use crate::pdf_model::{ExtractedDoc, FlowEvent};

const TITLE_FONT: &[u8] = include_bytes!("../assets/NotoSans-Bold.ttf");
const BODY_FONT: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

/// Cover art dimensions for the generated fallback: the 1.6 aspect ratio Kindle asks
/// for, at the resolution it recommends for the library thumbnail.
const GEN_WIDTH: u32 = 1600;
const GEN_HEIGHT: u32 = 2560;

/// Anything smaller than this on the opening page is an ornament or a publisher logo,
/// not the book's cover art.
const MIN_DETECT_WIDTH: u32 = 300;
const MIN_DETECT_HEIGHT: u32 = 400;

/// Below this Kindle renders a visibly soft thumbnail; worth a heads-up, but not worth
/// throwing away real cover art over.
const KINDLE_MIN_WIDTH: u32 = 625;
const KINDLE_MIN_HEIGHT: u32 = 1000;

pub struct Cover {
    pub filename: String,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Index into `ExtractedDoc::images` when the cover was lifted out of the PDF
    /// itself, so the same picture isn't repeated right after the cover page.
    pub consumed_image: Option<usize>,
}

/// Picks the cover art, in decreasing order of confidence: a file the user pointed at,
/// the largest image on the opening page, or a typographic cover drawn from the
/// metadata. The last step never fails, so every EPUB ends up with a cover.
pub fn select_cover(
    doc: &ExtractedDoc,
    explicit: Option<&Path>,
    title: &str,
    author: &str,
    mut log: impl FnMut(String),
) -> Cover {
    if let Some(path) = explicit {
        match from_file(path) {
            Ok(cover) => {
                log(format!(
                    "capa: arquivo informado {} ({}x{})",
                    path.display(),
                    cover.width,
                    cover.height
                ));
                return cover;
            }
            Err(e) => log(format!(
                "capa: não consegui usar {} ({e}), tentando detectar no PDF",
                path.display()
            )),
        }
    }

    if let Some(cover) = detect_in_pdf(doc, &mut log) {
        return cover;
    }

    let cover = generate(title, author);
    log(format!(
        "capa: nenhuma imagem adequada no PDF, gerando capa com título e autor ({}x{})",
        cover.width, cover.height
    ));
    cover
}

fn from_file(path: &Path) -> Result<Cover, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let decoded = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let (width, height) = (decoded.width(), decoded.height());

    // Pass JPEG/PNG through byte for byte; re-encode anything else so the EPUB only
    // ever carries the two formats every reader is guaranteed to handle.
    let format = image::guess_format(&bytes).map_err(|e| e.to_string())?;
    let (filename, mime, bytes) = match format {
        image::ImageFormat::Jpeg => ("cover.jpg".to_string(), "image/jpeg", bytes),
        image::ImageFormat::Png => ("cover.png".to_string(), "image/png", bytes),
        _ => ("cover.jpg".to_string(), "image/jpeg", encode_jpeg(&decoded.to_rgb8())?),
    };

    Ok(Cover { filename, mime, bytes, width, height, consumed_image: None })
}

/// Looks for cover art among the images drawn on the opening page, falling through to
/// the second page for PDFs that start with a blank or title-only sheet.
fn detect_in_pdf(doc: &ExtractedDoc, log: &mut impl FnMut(String)) -> Option<Cover> {
    for page in doc.pages.iter().take(2) {
        let best = page
            .events
            .iter()
            .filter_map(|(_, event)| match event {
                FlowEvent::Image(idx) => doc.images.get(*idx).map(|img| (*idx, img)),
                FlowEvent::Line(_) => None,
            })
            .max_by_key(|(_, img)| u64::from(img.width) * u64::from(img.height));

        let (idx, img) = match best {
            Some(found) => found,
            None => continue,
        };

        if img.width < MIN_DETECT_WIDTH || img.height < MIN_DETECT_HEIGHT {
            log(format!(
                "capa: maior imagem da página {} é pequena demais ({}x{}), ignorando",
                page.page_num, img.width, img.height
            ));
            continue;
        }

        let ext = if img.mime == "image/jpeg" { "jpg" } else { "png" };
        log(format!(
            "capa: usando a imagem {} da página {} ({}x{})",
            img.filename, page.page_num, img.width, img.height
        ));
        if img.width < KINDLE_MIN_WIDTH || img.height < KINDLE_MIN_HEIGHT {
            log(format!(
                "capa: atenção — abaixo de {KINDLE_MIN_WIDTH}x{KINDLE_MIN_HEIGHT}, \
                 a miniatura no Kindle pode sair borrada (use --cover para uma imagem maior)"
            ));
        }
        return Some(Cover {
            filename: format!("cover.{ext}"),
            mime: img.mime,
            bytes: img.bytes.clone(),
            width: img.width,
            height: img.height,
            consumed_image: Some(idx),
        });
    }
    None
}

/// Draws a plain typographic cover: title centred in the upper half, author below it.
fn generate(title: &str, author: &str) -> Cover {
    let mut canvas = RgbImage::from_pixel(GEN_WIDTH, GEN_HEIGHT, Rgb([0x1c, 0x24, 0x33]));

    let title_font = FontRef::try_from_slice(TITLE_FONT).expect("fonte de título embutida inválida");
    let body_font = FontRef::try_from_slice(BODY_FONT).expect("fonte de corpo embutida inválida");

    let margin = GEN_WIDTH / 8;
    let max_width = GEN_WIDTH - 2 * margin;

    // Shrink the title until it wraps into at most five lines *and* its longest single
    // word fits the column — otherwise a long unbreakable word would have to be split
    // mid-word to stay inside the art, which reads badly.
    let longest_word_fits = |size: f32| {
        let scaled = title_font.as_scaled(PxScale::from(size));
        title
            .split_whitespace()
            .all(|w| line_width(w, &scaled) <= max_width as f32)
    };
    let mut title_size = 140.0_f32;
    let mut title_lines = wrap_text(title, &title_font, title_size, max_width);
    while title_size > 40.0 && (title_lines.len() > 5 || !longest_word_fits(title_size)) {
        title_size -= 8.0;
        title_lines = wrap_text(title, &title_font, title_size, max_width);
    }

    let author_size = 72.0_f32;
    let author_lines = match author.trim() {
        "" => Vec::new(),
        author => wrap_text(author, &body_font, author_size, max_width),
    };

    // Centre the whole block a little above the middle — the classic cover balance,
    // and it keeps long titles from crowding either edge.
    let title_line_height = title_size * 1.3;
    let author_line_height = author_size * 1.3;
    let gap = if author_lines.is_empty() { 0.0 } else { title_line_height };
    let block_height = title_lines.len() as f32 * title_line_height
        + gap
        + author_lines.len() as f32 * author_line_height;
    let mut y = ((GEN_HEIGHT as f32 - block_height) / 2.0 * 0.85).max(margin as f32);

    for line in &title_lines {
        draw_centered_line(&mut canvas, line, &title_font, title_size, y, Rgb([0xf5, 0xf7, 0xfa]));
        y += title_line_height;
    }
    y += gap;
    for line in &author_lines {
        draw_centered_line(&mut canvas, line, &body_font, author_size, y, Rgb([0xa8, 0xb6, 0xcc]));
        y += author_line_height;
    }

    let bytes = encode_jpeg(&canvas).expect("codificação JPEG da capa gerada");
    Cover {
        filename: "cover.jpg".to_string(),
        mime: "image/jpeg",
        bytes,
        width: GEN_WIDTH,
        height: GEN_HEIGHT,
        consumed_image: None,
    }
}

/// Greedy word wrap that never returns a line wider than `max_width`: whole words
/// wherever possible, and a mid-word break only for a word that cannot fit a line on its
/// own even after the caller has shrunk the size as far as it will go.
fn wrap_text(text: &str, font: &FontRef, size: f32, max_width: u32) -> Vec<String> {
    let scaled = font.as_scaled(PxScale::from(size));
    let max = max_width as f32;
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        for piece in break_to_fit(word, &scaled, max) {
            let candidate = if current.is_empty() {
                piece.clone()
            } else {
                format!("{current} {piece}")
            };
            if line_width(&candidate, &scaled) <= max || current.is_empty() {
                current = candidate;
            } else {
                lines.push(std::mem::take(&mut current));
                current = piece;
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Splits an over-long word into the widest chunks that still fit. Words that already fit
/// come back untouched, which is the overwhelmingly common case.
fn break_to_fit<F: Font>(word: &str, scaled: &impl ScaleFont<F>, max: f32) -> Vec<String> {
    if line_width(word, scaled) <= max {
        return vec![word.to_string()];
    }
    let mut pieces = Vec::new();
    let mut chunk = String::new();
    for c in word.chars() {
        let candidate = format!("{chunk}{c}");
        if line_width(&candidate, scaled) <= max || chunk.is_empty() {
            chunk = candidate;
        } else {
            pieces.push(std::mem::take(&mut chunk));
            chunk = c.to_string();
        }
    }
    if !chunk.is_empty() {
        pieces.push(chunk);
    }
    pieces
}

fn line_width<F: Font>(text: &str, scaled: &impl ScaleFont<F>) -> f32 {
    let mut width = 0.0;
    let mut previous: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = previous {
            width += scaled.kern(prev, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

/// Rasterizes one line of text horizontally centred on the canvas, with `y` as the
/// baseline's ascent origin.
fn draw_centered_line(
    canvas: &mut RgbImage,
    text: &str,
    font: &FontRef,
    size: f32,
    y: f32,
    color: Rgb<u8>,
) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut pen_x = (canvas.width() as f32 - line_width(text, &scaled)) / 2.0;
    let baseline = y + scaled.ascent();
    let mut previous: Option<ab_glyph::GlyphId> = None;

    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = previous {
            pen_x += scaled.kern(prev, id);
        }
        let glyph: Glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen_x, baseline));
        pen_x += scaled.h_advance(id);
        previous = Some(id);

        let outlined = match font.outline_glyph(glyph) {
            Some(outlined) => outlined,
            None => continue, // whitespace and glyphs with no contours
        };
        let bounds = outlined.px_bounds();
        outlined.draw(|dx, dy, coverage| {
            let px = bounds.min.x as i32 + dx as i32;
            let py = bounds.min.y as i32 + dy as i32;
            if px < 0 || py < 0 || px >= canvas.width() as i32 || py >= canvas.height() as i32 {
                return;
            }
            let dst = canvas.get_pixel_mut(px as u32, py as u32);
            for ch in 0..3 {
                let blended = dst.0[ch] as f32 * (1.0 - coverage) + color.0[ch] as f32 * coverage;
                dst.0[ch] = blended.round().clamp(0.0, 255.0) as u8;
            }
        });
    }
}

fn encode_jpeg(img: &RgbImage) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut std::io::Cursor::new(&mut buf), 88)
        .encode_image(&image::DynamicImage::ImageRgb8(img.clone()))
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// The full-bleed cover page every major reader honours: an SVG viewport scaled to fit
/// the screen, so the art keeps its aspect ratio instead of being stretched.
pub fn cover_xhtml(cover: &Cover) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE html>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
<head><title>Capa</title><meta charset=\"utf-8\"/>\n\
<style>body{{margin:0;padding:0;}}</style></head>\n\
<body epub:type=\"cover\">\n\
<div style=\"text-align:center;page-break-after:always;\">\n\
<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
version=\"1.1\" width=\"100%\" height=\"100%\" viewBox=\"0 0 {w} {h}\" \
preserveAspectRatio=\"xMidYMid meet\">\n\
  <image width=\"{w}\" height=\"{h}\" xlink:href=\"{href}\"/>\n\
</svg>\n\
</div>\n\
</body>\n</html>\n",
        w = cover.width,
        h = cover.height,
        href = escape_html(&cover.filename),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{doc, jpeg, line, page, png, real_png_bytes};

    fn silent(_: String) {}

    #[test]
    fn an_explicit_file_wins_over_anything_in_the_pdf() {
        let dir = std::env::temp_dir().join("pdf_to_epub_cover_explicit");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capa.png");
        std::fs::write(&path, real_png_bytes(700, 1100)).unwrap();

        let d = doc(vec![page(1, vec![FlowEvent::Image(0)])], vec![jpeg("p1.jpg", 900, 1400)]);
        let cover = select_cover(&d, Some(&path), "T", "A", silent);

        assert_eq!((cover.width, cover.height), (700, 1100));
        assert_eq!(cover.mime, "image/png");
        assert_eq!(cover.consumed_image, None, "arquivo externo não consome imagem do PDF");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreadable_explicit_file_falls_back_to_detection_instead_of_failing() {
        let d = doc(vec![page(1, vec![FlowEvent::Image(0)])], vec![jpeg("p1.jpg", 900, 1400)]);
        let cover = select_cover(&d, Some(Path::new("/nao/existe/capa.jpg")), "T", "A", silent);

        assert_eq!((cover.width, cover.height), (900, 1400));
        assert_eq!(cover.consumed_image, Some(0));
    }

    #[test]
    fn detection_takes_the_largest_image_on_the_opening_page() {
        let d = doc(
            vec![page(1, vec![FlowEvent::Image(0), FlowEvent::Image(1), FlowEvent::Image(2)])],
            vec![jpeg("a.jpg", 400, 600), jpeg("b.jpg", 900, 1400), jpeg("c.jpg", 500, 800)],
        );
        let cover = select_cover(&d, None, "T", "A", silent);

        assert_eq!(cover.consumed_image, Some(1));
        assert_eq!(cover.filename, "cover.jpg");
    }

    #[test]
    fn a_png_cover_keeps_its_extension_and_mime() {
        let d = doc(vec![page(1, vec![FlowEvent::Image(0)])], vec![png("a.png", 900, 1400)]);
        let cover = select_cover(&d, None, "T", "A", silent);

        assert_eq!(cover.filename, "cover.png");
        assert_eq!(cover.mime, "image/png");
    }

    #[test]
    fn an_ornament_sized_image_is_not_mistaken_for_cover_art() {
        let d = doc(vec![page(1, vec![FlowEvent::Image(0)])], vec![jpeg("logo.jpg", 120, 60)]);
        let cover = select_cover(&d, None, "Título", "Autor", silent);

        assert_eq!(cover.consumed_image, None, "deve gerar a capa, não usar o logo");
        assert_eq!((cover.width, cover.height), (GEN_WIDTH, GEN_HEIGHT));
    }

    #[test]
    fn detection_falls_through_to_the_second_page() {
        let d = doc(
            vec![
                page(1, vec![line("Folha de rosto", 0.0, 12.0)]),
                page(2, vec![FlowEvent::Image(0)]),
            ],
            vec![jpeg("capa.jpg", 900, 1400)],
        );
        let cover = select_cover(&d, None, "T", "A", silent);

        assert_eq!(cover.consumed_image, Some(0));
    }

    #[test]
    fn an_image_deeper_in_the_book_is_never_used_as_cover() {
        let d = doc(
            vec![
                page(1, vec![line("Rosto", 0.0, 12.0)]),
                page(2, vec![line("Sumário", 0.0, 12.0)]),
                page(3, vec![FlowEvent::Image(0)]),
            ],
            vec![jpeg("meio.jpg", 900, 1400)],
        );
        let cover = select_cover(&d, None, "Título", "Autor", silent);

        assert_eq!(cover.consumed_image, None);
        assert_eq!((cover.width, cover.height), (GEN_WIDTH, GEN_HEIGHT));
    }

    #[test]
    fn a_document_with_no_images_at_all_still_gets_a_cover() {
        let d = doc(vec![page(1, vec![line("Só texto", 0.0, 12.0)])], vec![]);
        let cover = select_cover(&d, None, "Título", "Autor", silent);

        assert_eq!(cover.mime, "image/jpeg");
        assert!(!cover.bytes.is_empty());
        assert_eq!((cover.width, cover.height), (GEN_WIDTH, GEN_HEIGHT));
    }

    #[test]
    fn an_empty_document_does_not_panic() {
        let cover = select_cover(&doc(vec![], vec![]), None, "Título", "", silent);
        assert_eq!((cover.width, cover.height), (GEN_WIDTH, GEN_HEIGHT));
    }

    #[test]
    fn the_cover_page_declares_the_real_dimensions() {
        let cover = Cover {
            filename: "cover.png".to_string(),
            mime: "image/png",
            bytes: vec![],
            width: 640,
            height: 960,
            consumed_image: None,
        };
        let xhtml = cover_xhtml(&cover);

        assert!(xhtml.contains(r#"viewBox="0 0 640 960""#), "{xhtml}");
        assert!(xhtml.contains(r#"xlink:href="cover.png""#), "{xhtml}");
        assert!(xhtml.contains(r#"epub:type="cover""#), "{xhtml}");
    }

    #[test]
    fn the_cover_page_is_well_formed_xml() {
        // The cover page is hand-built rather than templated, so a stray character here
        // would break the whole EPUB for strict readers.
        let xhtml = cover_xhtml(&crate::test_support::some_cover());
        let mut depth = 0i32;
        for (i, _) in xhtml.match_indices("<svg") {
            assert!(xhtml[i..].contains("</svg>"));
            depth += 1;
        }
        assert_eq!(depth, 1);
        assert_eq!(xhtml.matches("<body").count(), xhtml.matches("</body>").count());
    }

    /// A title made of one unbreakable word can't be wrapped, so the only way to keep it
    /// inside the art is to shrink it until it fits the width.
    #[test]
    fn a_single_very_long_word_stays_inside_the_generated_cover() {
        let cover = generate("Pneumoultramicroscopicossilicovulcanoconiotico", "Autor");
        let img = image::load_from_memory(&cover.bytes).unwrap().to_rgb8();

        let margin = GEN_WIDTH / 16; // metade da margem de layout, para tolerar o JPEG
        let mut leftmost = GEN_WIDTH;
        let mut rightmost = 0;
        for (x, _y, px) in img.enumerate_pixels() {
            // fundo é escuro (0x1c2433); qualquer coisa clara é texto
            if px.0[0] > 90 && px.0[1] > 90 && px.0[2] > 90 {
                leftmost = leftmost.min(x);
                rightmost = rightmost.max(x);
            }
        }
        assert!(leftmost >= margin, "texto encosta na borda esquerda (x={leftmost})");
        assert!(
            rightmost <= GEN_WIDTH - margin,
            "texto vaza pela borda direita (x={rightmost}, largura={GEN_WIDTH})"
        );
    }

    #[test]
    fn wrapping_breaks_on_words_and_keeps_them_whole() {
        let font = FontRef::try_from_slice(TITLE_FONT).unwrap();
        let lines = wrap_text("um dois tres quatro cinco seis sete oito", &font, 140.0, 800);

        assert!(lines.len() > 1, "esperava quebra em várias linhas: {lines:?}");
        assert_eq!(lines.join(" "), "um dois tres quatro cinco seis sete oito");
    }

    #[test]
    fn wrapping_collapses_runs_of_whitespace() {
        let font = FontRef::try_from_slice(TITLE_FONT).unwrap();
        assert_eq!(wrap_text("  a \n\t b  ", &font, 40.0, 4000), vec!["a b".to_string()]);
    }
}
