mod chapters;
mod epub_gen;
mod images;
mod pdf_model;
mod pdf_parse;
mod preview;
mod tui;

use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().collect();

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
        if let Err(e) = run_headless(&pdf_path, &out_dir) {
            eprintln!("erro: {e}");
            std::process::exit(1);
        }
        return;
    }

    let initial_path = args.get(1).cloned();
    if let Err(e) = tui::run(initial_path) {
        eprintln!("erro: {e}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!("pdf_to_epub {} — conversor de PDF para EPUB com TUI", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USO:");
    println!("  pdf_to_epub [caminho.pdf]                 abre a interface interativa");
    println!("  pdf_to_epub --headless <caminho.pdf> [dir] roda sem interface (scripts/depuração)");
    println!("  pdf_to_epub --version                     mostra a versão");
    println!("  pdf_to_epub --help                        mostra esta ajuda");
}

/// Runs the full conversion pipeline without the TUI, logging each step to stdout.
/// Useful for scripting/debugging and for verifying the pipeline end-to-end.
fn run_headless(pdf_path: &Path, out_dir: &Path) -> Result<(), String> {
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

    let chapters = chapters::detect_chapters(&extracted, |s| println!("[capítulos] {s}"));
    println!("capítulos detectados: {}", chapters.len());

    let stem = pdf_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "livro".to_string());
    let epub_path = out_dir.join(format!("{stem}.epub"));
    let fragments = epub_gen::build_epub(&chapters, &extracted.images, &title, &author, "pt", &epub_path)?;
    println!("EPUB gerado em: {}", epub_path.display());

    let preview_path = preview::write_preview(&fragments, &extracted.images, &title, &author, out_dir)?;
    println!("preview gerado em: {}", preview_path.display());

    Ok(())
}
