use std::fs::File;
use std::path::Path;

use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};

use crate::chapters::Chapter;
use crate::cover::{self, Cover};
use crate::layout::{self, Block, LayoutMetrics};
use crate::pdf_model::{FlowEvent, ImageAsset};

/// The book's stylesheet. Without one the reader falls back to its own defaults and the
/// text loses every distinction the PDF made — paragraph indents, caption size, centring.
pub const STYLESHEET: &str = "\
body { margin: 0 5%; line-height: 1.5; text-align: justify; }
h1 { font-size: 1.5em; line-height: 1.25; text-align: left; margin: 2em 0 1em; }
h2 { font-size: 1.2em; line-height: 1.3; text-align: left; margin: 1.8em 0 0.6em; }
p { margin: 0; text-indent: 1.4em; }
h1 + p, h2 + p, img + p { text-indent: 0; }
p.legenda { font-size: 0.85em; text-align: center; text-indent: 0; margin: 0.4em 0 1.4em; }
p.direita { text-align: right; text-indent: 0; margin: 0.2em 0 1.2em; }
img { max-width: 100%; display: block; margin: 1.4em auto; }
";

/// (chapter title, body HTML fragment) — the same fragments written into the EPUB's
/// chapter files, reused verbatim by the HTML preview so both outputs always agree.
pub type ChapterHtml = (String, String);

pub fn build_epub(
    chapters: &[Chapter],
    images: &[ImageAsset],
    cover: &Cover,
    metrics: &LayoutMetrics,
    title: &str,
    author: &str,
    lang: &str,
    out_path: &Path,
) -> Result<Vec<ChapterHtml>, String> {
    let mut builder = EpubBuilder::new(ZipLibrary::new().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    builder.metadata("title", title).map_err(|e| e.to_string())?;
    if !author.trim().is_empty() {
        builder.metadata("author", author).map_err(|e| e.to_string())?;
    }
    builder.set_lang(lang);
    builder.stylesheet(STYLESHEET.as_bytes()).map_err(|e| e.to_string())?;

    // The cover has to go in before `inline_toc`: epub-builder appends to the spine in
    // call order, and the cover page must be the first thing the reader opens.
    builder
        .add_cover_image(&cover.filename, cover.bytes.as_slice(), cover.mime)
        .map_err(|e| e.to_string())?;
    builder
        .add_content(
            // Deliberately untitled: epub-builder only lists titled content in the ToC,
            // and "Capa" as a navigation entry is just noise.
            EpubContent::new("cover.xhtml", cover::cover_xhtml(cover).as_bytes())
                .reftype(ReferenceType::Cover),
        )
        .map_err(|e| e.to_string())?;

    builder.inline_toc();

    for (_, img) in resources_to_write(images, cover.consumed_image) {
        builder
            .add_resource(format!("images/{}", img.filename), img.bytes.as_slice(), img.mime)
            .map_err(|e| e.to_string())?;
    }

    let mut fragments = Vec::with_capacity(chapters.len());
    for chapter in chapters {
        let body = build_body_html(&chapter.events, images, cover.consumed_image, metrics);
        // A chapter whose body came out empty — most often one holding nothing but the
        // image just promoted to cover art — would be a dead entry in the table of
        // contents, so it is dropped rather than written out.
        if body.trim().is_empty() {
            continue;
        }
        let doc = xhtml_document(&chapter.title, &body);
        builder
            .add_content(
                EpubContent::new(
                    format!("chap_{:02}.xhtml", fragments.len() + 1),
                    doc.as_bytes(),
                )
                .title(&chapter.title)
                .reftype(ReferenceType::Text),
            )
            .map_err(|e| e.to_string())?;
        fragments.push((chapter.title.clone(), body));
    }

    let mut file = File::create(out_path).map_err(|e| e.to_string())?;
    builder.generate(&mut file).map_err(|e| e.to_string())?;
    Ok(fragments)
}

/// The assets that belong in the EPUB as resources, paired with their original index so
/// the chapter bodies keep referring to them correctly. The image promoted to cover art is
/// left out, and a filename that shows up more than once — the same XObject drawn on
/// several pages — is written only the first time.
fn resources_to_write(
    images: &[ImageAsset],
    skip_image: Option<usize>,
) -> Vec<(usize, &ImageAsset)> {
    let mut seen = std::collections::HashSet::new();
    images
        .iter()
        .enumerate()
        .filter(|(i, img)| skip_image != Some(*i) && seen.insert(img.filename.as_str()))
        .collect()
}

/// Recovers the chapter's visual structure and renders it. `skip_image` drops the image
/// promoted to cover art, so it isn't shown again a page after the cover itself.
pub fn build_body_html(
    events: &[FlowEvent],
    images: &[ImageAsset],
    skip_image: Option<usize>,
    metrics: &LayoutMetrics,
) -> String {
    let kept: Vec<FlowEvent> = events
        .iter()
        .filter(|e| !matches!(e, FlowEvent::Image(i) if skip_image == Some(*i)))
        .cloned()
        .collect();
    blocks_to_html(&layout::blocks(&kept, metrics), images)
}

fn blocks_to_html(blocks: &[Block], images: &[ImageAsset]) -> String {
    let mut html = String::new();
    for block in blocks {
        match block {
            Block::Heading(t) => push_tag(&mut html, "<h2>", t, "</h2>"),
            Block::Paragraph(t) => push_tag(&mut html, "<p>", t, "</p>"),
            Block::Caption(t) => push_tag(&mut html, "<p class=\"legenda\">", t, "</p>"),
            Block::RightAligned(t) => push_tag(&mut html, "<p class=\"direita\">", t, "</p>"),
            Block::Image(idx) => {
                if let Some(img) = images.get(*idx) {
                    html.push_str("<img src=\"images/");
                    html.push_str(&img.filename);
                    html.push_str("\" alt=\"\"/>\n");
                }
            }
        }
    }
    html
}

fn push_tag(html: &mut String, open: &str, text: &str, close: &str) {
    html.push_str(open);
    html.push_str(&escape_html(text));
    html.push_str(close);
    html.push('\n');
}

pub fn xhtml_document(title: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE html>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
<head><title>{}</title><meta charset=\"utf-8\"/></head>\n\
<body>\n<h1>{}</h1>\n{}\n</body>\n</html>\n",
        escape_html(title),
        escape_html(title),
        body
    )
}

/// Escapes text for XHTML. Double quotes are escaped too, because callers also use this
/// to guard attribute values (the cover page's `xlink:href`) — `&quot;` in a text node is
/// valid and renders identically, so one always-safe function beats two easily-confused ones.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::jpeg;

    #[test]
    fn escapes_xml_text_markers() {
        assert_eq!(escape_html("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }

    #[test]
    fn escapes_double_quotes_for_attribute_use() {
        // `escape_html` is what guards the `xlink:href` attribute in cover.rs, so an
        // unescaped double quote there would break out of the attribute.
        assert_eq!(escape_html(r#"a"b"#), "a&quot;b");
    }

    fn render(blocks: Vec<Block>, images: &[ImageAsset]) -> String {
        blocks_to_html(&blocks, images)
    }

    #[test]
    fn each_kind_of_block_gets_its_own_markup() {
        let images = [jpeg("foto.jpg", 100, 100)];
        let html = render(
            vec![
                Block::Heading("Um subtítulo".into()),
                Block::Paragraph("Um parágrafo.".into()),
                Block::Image(0),
                Block::Caption("Figura 1".into()),
                Block::RightAligned("— Disraeli".into()),
            ],
            &images,
        );
        assert_eq!(
            html,
            "<h2>Um subtítulo</h2>\n\
             <p>Um parágrafo.</p>\n\
             <img src=\"images/foto.jpg\" alt=\"\"/>\n\
             <p class=\"legenda\">Figura 1</p>\n\
             <p class=\"direita\">— Disraeli</p>\n"
        );
    }

    #[test]
    fn block_text_is_escaped() {
        let html = render(vec![Block::Paragraph("a & b < c".into())], &[]);
        assert_eq!(html, "<p>a &amp; b &lt; c</p>\n");
    }

    #[test]
    fn an_image_index_with_no_asset_is_skipped_silently() {
        let html = render(vec![Block::Paragraph("Texto.".into()), Block::Image(7)], &[]);
        assert_eq!(html, "<p>Texto.</p>\n");
    }

    #[test]
    fn the_stylesheet_styles_every_class_the_body_can_emit() {
        let css = STYLESHEET;
        for needed in ["h1", "h2", "p.legenda", "p.direita", "img"] {
            assert!(css.contains(needed), "faltou regra para {needed} em:\n{css}");
        }
    }

    fn written(images: &[ImageAsset], skip: Option<usize>) -> Vec<String> {
        resources_to_write(images, skip)
            .into_iter()
            .map(|(_, a)| a.filename.clone())
            .collect()
    }

    /// The same XObject drawn on several pages is extracted once per page, so it arrives
    /// here as several assets sharing one filename. Writing it repeatedly would put
    /// duplicate entries in the zip and duplicate ids in the manifest.
    #[test]
    fn a_picture_repeated_across_pages_is_written_to_the_epub_once() {
        let images = [
            jpeg("img_9_0.jpg", 500, 500),
            jpeg("outra.jpg", 100, 100),
            jpeg("img_9_0.jpg", 500, 500),
        ];
        assert_eq!(written(&images, None), vec!["img_9_0.jpg", "outra.jpg"]);
    }

    #[test]
    fn the_image_promoted_to_cover_is_not_written_twice() {
        let images = [jpeg("capa.jpg", 900, 1400), jpeg("foto.jpg", 100, 100)];
        assert_eq!(written(&images, Some(0)), vec!["foto.jpg"]);
    }

    #[test]
    fn resources_keep_their_original_indices_for_body_references() {
        let images = [jpeg("a.jpg", 10, 10), jpeg("b.jpg", 10, 10)];
        let indices: Vec<usize> = resources_to_write(&images, Some(0))
            .into_iter()
            .map(|(i, _)| i)
            .collect();
        assert_eq!(indices, vec![1]);
    }

    #[test]
    fn chapter_title_is_escaped_in_both_head_and_heading() {
        let doc = xhtml_document("Fome & Sede", "<p>oi</p>");
        assert!(doc.contains("<title>Fome &amp; Sede</title>"), "{doc}");
        assert!(doc.contains("<h1>Fome &amp; Sede</h1>"), "{doc}");
    }
}
