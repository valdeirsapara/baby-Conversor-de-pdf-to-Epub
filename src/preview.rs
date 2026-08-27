use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;

use crate::epub_gen::escape_html;
use crate::pdf_model::ImageAsset;

/// Builds a single self-contained preview.html reusing the exact same chapter HTML
/// fragments written into the EPUB, with every image inlined as a data: URI so the
/// file can be opened directly with no relative-path dependencies.
pub fn write_preview(
    chapters_html: &[(String, String)],
    images: &[ImageAsset],
    title: &str,
    author: &str,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    let mut body = String::new();
    body.push_str(&format!(
        "<h1>{}</h1>\n<p><em>{}</em></p>\n",
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
img{{max-width:100%;display:block;margin:1rem auto}}h2{{border-top:1px solid #ccc;padding-top:1rem}}</style>\
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
        let b64 = base64::engine::general_purpose::STANDARD.encode(&img.bytes);
        let data_uri = format!("data:{};base64,{}", img.mime, b64);
        out = out.replace(&needle, &data_uri);
    }
    out
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
