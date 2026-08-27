use std::fs::File;
use std::path::Path;

use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};

use crate::chapters::Chapter;
use crate::pdf_model::{FlowEvent, ImageAsset};

/// (chapter title, body HTML fragment) — the same fragments written into the EPUB's
/// chapter files, reused verbatim by the HTML preview so both outputs always agree.
pub type ChapterHtml = (String, String);

pub fn build_epub(
    chapters: &[Chapter],
    images: &[ImageAsset],
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
    builder.inline_toc();

    for img in images {
        builder
            .add_resource(format!("images/{}", img.filename), img.bytes.as_slice(), img.mime)
            .map_err(|e| e.to_string())?;
    }

    let mut fragments = Vec::with_capacity(chapters.len());
    for (i, chapter) in chapters.iter().enumerate() {
        let body = build_body_html(&chapter.events, images);
        let doc = xhtml_document(&chapter.title, &body);
        builder
            .add_content(
                EpubContent::new(format!("chap_{:02}.xhtml", i + 1), doc.as_bytes())
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

pub fn build_body_html(events: &[FlowEvent], images: &[ImageAsset]) -> String {
    let mut html = String::new();
    let mut para = String::new();
    for event in events {
        match event {
            FlowEvent::Line(run) => {
                if !para.is_empty() {
                    para.push(' ');
                }
                para.push_str(&run.text);
            }
            FlowEvent::Image(idx) => {
                flush_paragraph(&mut para, &mut html);
                if let Some(img) = images.get(*idx) {
                    html.push_str("<img src=\"images/");
                    html.push_str(&img.filename);
                    html.push_str("\" alt=\"\"/>\n");
                }
            }
        }
    }
    flush_paragraph(&mut para, &mut html);
    html
}

fn flush_paragraph(para: &mut String, html: &mut String) {
    let t = para.trim();
    if !t.is_empty() {
        html.push_str("<p>");
        html.push_str(&escape_html(t));
        html.push_str("</p>\n");
    }
    para.clear();
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

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}
