use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::{chapters, cover, epub_gen, layout, pdf_parse, preview};

enum ProgressMsg {
    Stage(String),
    Log(String),
    Done {
        epub_path: PathBuf,
        preview_path: PathBuf,
        chapter_count: usize,
        image_count: usize,
    },
    Error(String),
}

type Done = Option<Result<(PathBuf, PathBuf, usize, usize), String>>;

enum Screen {
    Input {
        path: String,
        /// Carried over from `--cover` so it prefills the Confirm screen.
        cover: String,
    },
    Confirm {
        path: PathBuf,
        title: String,
        author: String,
        lang: String,
        /// Empty means "detect the cover automatically".
        cover: String,
        field: usize,
    },
    Progress {
        rx: mpsc::Receiver<ProgressMsg>,
        stage: String,
        logs: VecDeque<String>,
        done: Done,
    },
}

pub fn run(initial_path: Option<String>, initial_cover: Option<String>) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let mut screen = Screen::Input {
        path: initial_path.unwrap_or_default(),
        cover: initial_cover.unwrap_or_default(),
    };
    let result = event_loop(&mut terminal, &mut screen);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, screen: &mut Screen) -> std::io::Result<()> {
    loop {
        terminal.draw(|f| draw(f, screen))?;

        if let Screen::Progress { rx, stage, logs, done } = screen {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    ProgressMsg::Stage(s) => *stage = s,
                    ProgressMsg::Log(s) => {
                        logs.push_back(s);
                        if logs.len() > 500 {
                            logs.pop_front();
                        }
                    }
                    ProgressMsg::Done {
                        epub_path,
                        preview_path,
                        chapter_count,
                        image_count,
                    } => {
                        *done = Some(Ok((epub_path, preview_path, chapter_count, image_count)));
                    }
                    ProgressMsg::Error(e) => *done = Some(Err(e)),
                }
            }
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && handle_key(key.code, screen) {
                    return Ok(());
                }
            }
        }
    }
}

fn handle_key(code: KeyCode, screen: &mut Screen) -> bool {
    match screen {
        Screen::Input { path, cover } => match code {
            KeyCode::Enter => {
                let trimmed = path.trim().to_string();
                if !trimmed.is_empty() && Path::new(&trimmed).exists() {
                    let pb = PathBuf::from(&trimmed);
                    let (title, author) = quick_scan_metadata(&pb);
                    *screen = Screen::Confirm {
                        path: pb,
                        title,
                        author,
                        lang: "pt".to_string(),
                        cover: cover.clone(),
                        field: 0,
                    };
                }
            }
            KeyCode::Char(c) => path.push(c),
            KeyCode::Backspace => {
                path.pop();
            }
            KeyCode::Esc => return true,
            _ => {}
        },
        Screen::Confirm {
            path,
            title,
            author,
            lang,
            cover,
            field,
        } => match code {
            KeyCode::Tab => *field = (*field + 1) % 4,
            KeyCode::BackTab => *field = (*field + 3) % 4,
            KeyCode::Enter => {
                let (tx, rx) = mpsc::channel();
                let pdf_path = path.clone();
                let out_dir = pdf_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."));
                let doc_title = title.clone();
                let doc_author = author.clone();
                let doc_lang = lang.clone();
                let doc_cover = cover.trim().to_string();
                thread::spawn(move || {
                    run_pipeline(pdf_path, out_dir, doc_title, doc_author, doc_lang, doc_cover, tx)
                });
                *screen = Screen::Progress {
                    rx,
                    stage: "Iniciando".to_string(),
                    logs: VecDeque::new(),
                    done: None,
                };
            }
            KeyCode::Char(c) => match field {
                0 => title.push(c),
                1 => author.push(c),
                2 => lang.push(c),
                _ => cover.push(c),
            },
            KeyCode::Backspace => {
                match field {
                    0 => {
                        title.pop();
                    }
                    1 => {
                        author.pop();
                    }
                    2 => {
                        lang.pop();
                    }
                    _ => {
                        cover.pop();
                    }
                };
            }
            KeyCode::Esc => return true,
            _ => {}
        },
        Screen::Progress { done, .. } => match code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('p') => {
                if let Some(Ok((_, preview_path, _, _))) = done {
                    preview::open_in_browser(preview_path);
                }
            }
            KeyCode::Char('o') => {
                if let Some(Ok((epub_path, _, _, _))) = done {
                    if let Some(dir) = epub_path.parent() {
                        preview::open_in_browser(dir);
                    }
                }
            }
            KeyCode::Char('n') => {
                *screen = Screen::Input { path: String::new(), cover: String::new() };
            }
            KeyCode::Esc => return true,
            _ => {}
        },
    }
    false
}

fn quick_scan_metadata(path: &Path) -> (String, String) {
    if let Ok(doc) = lopdf::Document::load(path) {
        let (title, author) = pdf_parse::info_metadata(&doc);
        return (title.unwrap_or_else(|| filename_title(path)), author.unwrap_or_default());
    }
    (filename_title(path), String::new())
}

fn filename_title(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Documento".to_string())
}

fn epub_filename(pdf_path: &Path) -> String {
    let stem = pdf_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "livro".to_string());
    format!("{stem}.epub")
}

fn run_pipeline(
    pdf_path: PathBuf,
    out_dir: PathBuf,
    title: String,
    author: String,
    lang: String,
    cover_path: String,
    tx: mpsc::Sender<ProgressMsg>,
) {
    let _ = tx.send(ProgressMsg::Stage("Extraindo texto e imagens do PDF".to_string()));
    let tx_log = tx.clone();
    let extracted = match pdf_parse::parse_document(&pdf_path, move |s| {
        let _ = tx_log.send(ProgressMsg::Log(s));
    }) {
        Ok(d) => d,
        Err(e) => {
            let _ = tx.send(ProgressMsg::Error(format!("Falha ao ler o PDF: {e}")));
            return;
        }
    };

    let _ = tx.send(ProgressMsg::Stage("Escolhendo a capa".to_string()));
    let tx_log = tx.clone();
    let cover = cover::select_cover(
        &extracted,
        (!cover_path.is_empty()).then(|| Path::new(&cover_path)),
        &title,
        &author,
        move |s| {
            let _ = tx_log.send(ProgressMsg::Log(s));
        },
    );

    let metrics = layout::measure(&extracted);
    let _ = tx.send(ProgressMsg::Log(format!(
        "layout: corpo {:.1}pt, entrelinha {:.1}pt",
        metrics.baseline_font, metrics.line_gap
    )));

    let _ = tx.send(ProgressMsg::Stage("Detectando capítulos".to_string()));
    let tx_log = tx.clone();
    let chapters = chapters::detect_chapters(&extracted, move |s| {
        let _ = tx_log.send(ProgressMsg::Log(s));
    });

    let _ = tx.send(ProgressMsg::Stage("Gerando EPUB".to_string()));
    let epub_path = out_dir.join(epub_filename(&pdf_path));
    let fragments = match epub_gen::build_epub(
        &chapters,
        &extracted.images,
        &cover,
        &metrics,
        &title,
        &author,
        &lang,
        &epub_path,
    ) {
        Ok(f) => f,
        Err(e) => {
            let _ = tx.send(ProgressMsg::Error(format!("Falha ao gerar o EPUB: {e}")));
            return;
        }
    };

    let _ = tx.send(ProgressMsg::Stage("Gerando preview HTML".to_string()));
    let preview_path = match preview::write_preview(
        &fragments,
        &extracted.images,
        &cover,
        &title,
        &author,
        &out_dir,
    ) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(ProgressMsg::Error(format!("Falha ao gerar o preview: {e}")));
            return;
        }
    };

    let _ = tx.send(ProgressMsg::Done {
        epub_path,
        preview_path,
        chapter_count: chapters.len(),
        image_count: extracted.images.len(),
    });
}

fn draw(f: &mut Frame, screen: &Screen) {
    match screen {
        Screen::Input { path, .. } => draw_input(f, path),
        Screen::Confirm {
            title, author, lang, cover, field, ..
        } => draw_confirm(f, title, author, lang, cover, *field),
        Screen::Progress { stage, logs, done, .. } => draw_progress(f, stage, logs, done),
    }
}

fn draw_input(f: &mut Frame, path: &str) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    f.render_widget(
        Paragraph::new("pdf_to_epub — conversor de PDF para EPUB")
            .block(Block::default().borders(Borders::ALL).title("pdf_to_epub")),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(format!("{path}_")).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Caminho do PDF (Enter confirma, Esc sai)"),
        ),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new("Digite ou cole o caminho completo de qualquer arquivo .pdf.").wrap(Wrap { trim: true }),
        chunks[2],
    );
}

fn draw_confirm(f: &mut Frame, title: &str, author: &str, lang: &str, cover: &str, field: usize) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);
    let field_block = |label: &str, value: &str, active: bool| {
        let style = if active {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        Paragraph::new(value.to_string())
            .style(style)
            .block(Block::default().borders(Borders::ALL).title(label.to_string()))
    };
    f.render_widget(field_block("Título (Tab troca de campo)", title, field == 0), chunks[0]);
    f.render_widget(field_block("Autor", author, field == 1), chunks[1]);
    f.render_widget(field_block("Idioma (ISO, ex: pt, en)", lang, field == 2), chunks[2]);
    f.render_widget(
        field_block("Capa (caminho da imagem; vazio = detectar automaticamente)", cover, field == 3),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new("Enter inicia a conversão. Esc sai.").wrap(Wrap { trim: true }),
        chunks[4],
    );
}

fn draw_progress(f: &mut Frame, stage: &str, logs: &VecDeque<String>, done: &Done) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    let stage_text = match done {
        Some(Ok((epub, _, chaps, imgs))) => {
            format!("Concluído: {} ({chaps} capítulos, {imgs} imagens)", epub.display())
        }
        Some(Err(e)) => format!("Erro: {e}"),
        None => stage.to_string(),
    };
    f.render_widget(
        Paragraph::new(stage_text).block(Block::default().borders(Borders::ALL).title("Progresso")),
        chunks[0],
    );

    let items: Vec<ListItem> = logs.iter().rev().map(|l| ListItem::new(l.clone())).collect();
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Log")),
        chunks[1],
    );

    let footer = match done {
        Some(Ok(_)) => "[p] abrir preview no navegador  [o] abrir pasta de saída  [n] converter outro PDF  [q] sair",
        Some(Err(_)) => "[n] tentar outro PDF  [q] sair",
        None => "convertendo... [q] sair",
    };
    f.render_widget(Paragraph::new(footer), chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epub_sits_next_to_the_pdf_under_the_same_name() {
        assert_eq!(epub_filename(Path::new("/livros/Meu Livro.pdf")), "Meu Livro.epub");
    }

    #[test]
    fn a_dotted_filename_only_loses_its_last_extension() {
        assert_eq!(epub_filename(Path::new("vol.2.final.pdf")), "vol.2.final.epub");
    }

    #[test]
    fn the_title_falls_back_to_the_filename_without_its_extension() {
        assert_eq!(filename_title(Path::new("/tmp/Como Mentir.pdf")), "Como Mentir");
    }

    #[test]
    fn a_path_with_no_filename_still_yields_usable_names() {
        assert_eq!(filename_title(Path::new("/")), "Documento");
        assert_eq!(epub_filename(Path::new("/")), "livro.epub");
    }
}
