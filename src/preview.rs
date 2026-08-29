use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;

use crate::cover::Cover;
use crate::epub_gen::escape_html;
use crate::pdf_model::ImageAsset;

/// Builds a single self-contained preview.html reusing the exact same chapter HTML
/// fragments written into the EPUB, with every image inlined as a data: URI so the
/// file can be opened directly with no relative-path dependencies.
pub fn write_preview(
    chapters_html: &[(String, String)],
    images: &[ImageAsset],
    cover: &Cover,
    title: &str,
    author: &str,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    let mut body = String::new();
    body.push_str(&format!(
        "<img class=\"cover\" src=\"{}\" alt=\"Capa\"/>\n<h1>{}</h1>\n<p><em>{}</em></p>\n",
        data_uri(&cover.bytes, cover.mime),
        escape_html(title),
        escape_html(author)
    ));
    for (i, (chap_title, html)) in chapters_html.iter().enumerate() {
        body.push_str(&format!("<h2 id=\"chap{}\">{}</h2>\n", i + 1, escape_html(chap_title)));
        body.push_str(html);
        body.push_str("\n<hr/>\n");
    }
    let body = inline_images(&body, images);

    let page = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Preview: {}</title>\
<style>body{{max-width:700px;margin:2rem auto;font-family:Georgia,serif;line-height:1.5;padding:0 1rem}}\
img{{max-width:100%;display:block;margin:1rem auto}}\
img.cover{{max-height:80vh;box-shadow:0 2px 12px rgba(0,0,0,.3)}}h2{{border-top:1px solid #ccc;padding-top:1rem}}</style>\
</head><body>{}</body></html>",
        escape_html(title),
        body
    );

    let out_path = out_dir.join("preview.html");
    std::fs::write(&out_path, page).map_err(|e| e.to_string())?;
    Ok(out_path)
}

fn inline_images(html: &str, images: &[ImageAsset]) -> String {
    let mut out = html.to_string();
    for img in images {
        let needle = format!("images/{}", img.filename);
        out = out.replace(&needle, &data_uri(&img.bytes, img.mime));
    }
    out
}

fn data_uri(bytes: &[u8], mime: &str) -> String {
    format!("data:{};base64,{}", mime, base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Opens a file or directory with the Windows default handler via WSL interop
/// (`wslpath` + `explorer.exe`). Best-effort: failures are silently ignored since this
/// is a convenience action, not something the conversion depends on.
pub fn open_in_browser(path: &Path) {
    let win_path = Command::new("wslpath").arg("-w").arg(path).output();
    let target = match win_path {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => path.display().to_string(),
    };
    let _ = Command::new("/mnt/c/Windows/explorer.exe").arg(&target).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{jpeg, png};

    #[test]
    fn a_data_uri_carries_the_declared_mime_type() {
        assert_eq!(data_uri(b"abc", "image/png"), "data:image/png;base64,YWJj");
    }

    #[test]
    fn every_image_reference_is_replaced_by_its_data_uri() {
        let images = [jpeg("a.jpg", 10, 10), png("b.png", 10, 10)];
        let html = r#"<img src="images/a.jpg"/><img src="images/b.png"/>"#;
        let out = inline_images(html, &images);

        assert!(!out.contains("images/"), "sobrou caminho relativo: {out}");
        assert_eq!(out.matches("data:image/jpeg;base64,").count(), 1);
        assert_eq!(out.matches("data:image/png;base64,").count(), 1);
    }

    #[test]
    fn the_same_image_used_twice_is_inlined_in_both_places() {
        let images = [jpeg("a.jpg", 10, 10)];
        let out = inline_images(r#"<img src="images/a.jpg"/><img src="images/a.jpg"/>"#, &images);
        assert_eq!(out.matches("data:image/jpeg").count(), 2);
    }

    #[test]
    fn an_image_the_html_never_references_changes_nothing() {
        let images = [jpeg("orfa.jpg", 10, 10)];
        assert_eq!(inline_images("<p>oi</p>", &images), "<p>oi</p>");
    }
}
