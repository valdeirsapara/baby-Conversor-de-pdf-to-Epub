mod chapters;
mod cover;
mod epub_gen;
mod images;
mod layout;
mod pdf_model;
mod pdf_parse;
mod preview;
#[cfg(test)]
mod test_support;
mod tui;

use std::path::{Path, PathBuf};

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    // `--cover` is pulled out first so it can sit anywhere on the line; everything else
    // is still matched positionally against args[1].
    let cover_path = match take_cover_flag(&mut args) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("erro: {e}");
            std::process::exit(1);
        }
    };

    if args.get(1).map(String::as_str) == Some("--version") {
        println!("pdf_to_epub {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if args.get(1).map(String::as_str) == Some("--help") {
        print_help();
        return;
    }

    if args.get(1).map(String::as_str) == Some("--headless") {
        let pdf_path = match args.get(2) {
            Some(p) => PathBuf::from(p),
            None => {
                eprintln!("uso: pdf_to_epub --headless <arquivo.pdf> [dir_saida]");
                std::process::exit(1);
            }
        };
        let out_dir = args
            .get(3)
            .map(PathBuf::from)
            .or_else(|| pdf_path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        if let Err(e) = run_headless(&pdf_path, &out_dir, cover_path.as_deref()) {
            eprintln!("erro: {e}");
            std::process::exit(1);
        }
        return;
    }

    let initial_path = args.get(1).cloned();
    if let Err(e) = tui::run(initial_path, cover_path) {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
}

/// Removes `--cover <caminho>` from `args`, returning the path when present. A missing
/// value, a value that is itself a flag, or a repeated `--cover` are all rejected here
/// rather than being quietly absorbed into the positional arguments further down.
fn take_cover_flag(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let Some(i) = args.iter().position(|a| a == "--cover") else {
        return Ok(None);
    };
    if args.iter().filter(|a| *a == "--cover").count() > 1 {
        return Err("--cover foi informado mais de uma vez".to_string());
    }
    let path = match args.get(i + 1) {
        Some(v) if !v.starts_with("--") => v.clone(),
        Some(v) => {
            return Err(format!(
                "--cover esperava o caminho de uma imagem, mas veio a opção {v} \
                 (se for mesmo um arquivo, escreva ./{v})"
            ));
        }
        None => return Err("--cover exige o caminho de uma imagem".to_string()),
    };
    args.drain(i..=i + 1);
    Ok(Some(path))
}

fn print_help() {
    println!("pdf_to_epub {} — conversor de PDF para EPUB com TUI", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USO:");
    println!("  pdf_to_epub [caminho.pdf]                 abre a interface interativa");
    println!("  pdf_to_epub --headless <caminho.pdf> [dir] roda sem interface (scripts/depuração)");
    println!("  pdf_to_epub --version                     mostra a versão");
    println!("  pdf_to_epub --help                        mostra esta ajuda");
    println!();
    println!("OPÇÕES:");
    println!("  --cover <imagem>  usa esta imagem como capa. Sem isso, a capa é detectada na");
    println!("                    primeira página do PDF ou gerada com o título e o autor.");
}

/// Runs the full conversion pipeline without the TUI, logging each step to stdout.
/// Useful for scripting/debugging and for verifying the pipeline end-to-end.
fn run_headless(pdf_path: &Path, out_dir: &Path, cover_path: Option<&str>) -> Result<(), String> {
    let doc = lopdf::Document::load(pdf_path).map_err(|e| e.to_string())?;
    let (title, author) = pdf_parse::info_metadata(&doc);
    let title = title.unwrap_or_else(|| {
        pdf_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Documento".to_string())
    });
    let author = author.unwrap_or_default();
    drop(doc);
    println!("título: {title}");
    println!("autor: {author}");

    let extracted = pdf_parse::parse_document(pdf_path, |s| println!("[parse] {s}"))?;
    println!(
        "extração concluída: {} páginas, {} imagens",
        extracted.pages.len(),
        extracted.images.len()
    );

    let cover = cover::select_cover(
        &extracted,
        cover_path.map(Path::new),
        &title,
        &author,
        |s| println!("[capa] {s}"),
    );

    let metrics = layout::measure(&extracted);
    println!(
        "layout: corpo {:.1}pt, margem em {:.0}% da página, entrelinha {:.1}pt",
        metrics.baseline_font,
        metrics.left_margin * 100.0,
        metrics.line_gap
    );

    let chapters = chapters::detect_chapters(&extracted, |s| println!("[capítulos] {s}"));
    println!("capítulos detectados: {}", chapters.len());

    let stem = pdf_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "livro".to_string());
    let epub_path = out_dir.join(format!("{stem}.epub"));
    let fragments = epub_gen::build_epub(
        &chapters,
        &extracted.images,
        &cover,
        &metrics,
        &title,
        &author,
        "pt",
        &epub_path,
    )?;
    println!("EPUB gerado em: {}", epub_path.display());

    let preview_path =
        preview::write_preview(&fragments, &extracted.images, &cover, &title, &author, out_dir)?;
    println!("preview gerado em: {}", preview_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn without_the_flag_nothing_is_consumed() {
        let mut args = argv(&["pdf_to_epub", "livro.pdf"]);
        assert_eq!(take_cover_flag(&mut args), Ok(None));
        assert_eq!(args, argv(&["pdf_to_epub", "livro.pdf"]));
    }

    #[test]
    fn the_flag_is_removed_with_its_value_wherever_it_sits() {
        let mut args = argv(&["pdf_to_epub", "--cover", "capa.jpg", "--headless", "livro.pdf"]);
        assert_eq!(take_cover_flag(&mut args), Ok(Some("capa.jpg".to_string())));
        assert_eq!(args, argv(&["pdf_to_epub", "--headless", "livro.pdf"]));

        let mut args = argv(&["pdf_to_epub", "--headless", "livro.pdf", "saida", "--cover", "capa.jpg"]);
        assert_eq!(take_cover_flag(&mut args), Ok(Some("capa.jpg".to_string())));
        assert_eq!(args, argv(&["pdf_to_epub", "--headless", "livro.pdf", "saida"]));
    }

    #[test]
    fn the_flag_without_a_value_is_an_error() {
        let mut args = argv(&["pdf_to_epub", "livro.pdf", "--cover"]);
        assert!(take_cover_flag(&mut args).is_err());
    }

    #[test]
    fn the_next_flag_is_never_swallowed_as_the_cover_path() {
        // `--cover --headless livro.pdf` is a typo, not a request to open a file called
        // "--headless"; silently consuming it would drop the user into the TUI instead.
        let mut args = argv(&["pdf_to_epub", "--cover", "--headless", "livro.pdf"]);
        assert!(take_cover_flag(&mut args).is_err());
    }

    #[test]
    fn repeating_the_flag_is_an_error_rather_than_a_silent_pick() {
        let mut args = argv(&["pdf_to_epub", "--cover", "a.jpg", "--cover", "b.jpg", "livro.pdf"]);
        assert!(take_cover_flag(&mut args).is_err());
    }

    #[test]
    fn a_path_that_merely_looks_like_a_flag_still_works_after_a_separator() {
        // Um caminho relativo comum não deve ser confundido com flag.
        let mut args = argv(&["pdf_to_epub", "--cover", "./--estranho.jpg", "livro.pdf"]);
        assert_eq!(take_cover_flag(&mut args), Ok(Some("./--estranho.jpg".to_string())));
    }
}
