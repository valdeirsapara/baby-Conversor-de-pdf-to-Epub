//! Infers the document's visual structure from the geometry pdf-extract hands us.
//!
//! A PDF carries no notion of "paragraph" or "heading" — only glyphs at coordinates. Three
//! measurements survive extraction reliably, and between them they recover most of what a
//! reader actually sees: each line's **font size**, the **x where the line starts**, and the
//! **vertical distance to the line before it**.
//!
//! What does *not* survive is where a line *ends*. pdf-extract reports a glyph advance of
//! zero for the overwhelming majority of characters (99.87% of them on the reference book),
//! and the text matrix it hands us is per text-showing operation rather than per glyph — so
//! the rightmost x observable is merely where the line's last chunk began, which depends on
//! how the PDF happened to split the line. Nothing here may depend on line width: no
//! justification test, and no centring detection, since a centred line cannot be told from
//! a right-aligned one without knowing how wide it is. Guessing there produces confidently
//! wrong markup, so it is left undone.

use crate::pdf_model::{ExtractedDoc, FlowEvent, TextRun};

/// A line starting at least this far right of the margin (as a page fraction) is opening a
/// new paragraph. On the reference book the margin sits at 0.126 and indented lines at
/// 0.155, so the gap to clear is comfortably wide.
const INDENT_MIN: f64 = 0.012;
/// Vertical distance, as a multiple of the usual line spacing, that marks a paragraph break
/// rather than simply the next line of the same paragraph.
const PARAGRAPH_GAP: f64 = 1.35;
/// A line starting past this fraction of the page width is set flush right, not indented.
const RIGHT_ALIGN_MIN_X: f64 = 0.45;
/// Font size, relative to the body baseline, at which a short line reads as a subheading.
const HEADING_MIN: f64 = 1.10;
/// Font size, relative to the baseline, at which a line reads as caption-sized.
const CAPTION_MAX: f64 = 0.92;
/// Longest a line can be and still plausibly be a heading rather than a paragraph.
const HEADING_MAX_CHARS: usize = 80;
/// Longest a line can be and still plausibly be an attribution rather than a stray indent.
const RIGHT_ALIGN_MAX_CHARS: usize = 60;

/// Page-level statistics the per-line classification is measured against. Taken over the
/// whole document so every chapter is judged by the same ruler.
#[derive(Debug, Clone)]
pub struct LayoutMetrics {
    pub baseline_font: f64,
    /// Left edge of the text column, as a fraction of the page width.
    pub left_margin: f64,
    /// The usual vertical distance between consecutive lines of one paragraph.
    pub line_gap: f64,
}

impl Default for LayoutMetrics {
    fn default() -> Self {
        LayoutMetrics { baseline_font: 1.0, left_margin: 0.0, line_gap: 1.0 }
    }
}

/// One piece of rendered structure. Everything the EPUB writes out is one of these.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(String),
    Paragraph(String),
    Caption(String),
    RightAligned(String),
    Image(usize),
}

pub fn measure(doc: &ExtractedDoc) -> LayoutMetrics {
    let runs: Vec<&TextRun> = doc
        .pages
        .iter()
        .flat_map(|p| p.events.iter())
        .filter_map(|(_, e)| match e {
            FlowEvent::Line(run) => Some(run),
            FlowEvent::Image(_) => None,
        })
        .filter(|run| !run.text.trim().is_empty())
        .collect();

    if runs.is_empty() {
        return LayoutMetrics::default();
    }

    let baseline_font = percentile(runs.iter().map(|r| r.font_size), 0.5).max(f64::MIN_POSITIVE);
    // A percentile rather than the minimum: one stray glyph in the gutter shouldn't
    // redefine where the text column starts.
    let left_margin = percentile(runs.iter().map(|r| r.x_start), 0.10);

    // Only gaps within a page mean anything — across a page break y restarts at the top.
    let mut gaps: Vec<f64> = Vec::new();
    for page in &doc.pages {
        let ys: Vec<f64> = page
            .events
            .iter()
            .filter_map(|(_, e)| match e {
                FlowEvent::Line(run) if !run.text.trim().is_empty() => Some(run.y),
                _ => None,
            })
            .collect();
        gaps.extend(ys.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 0.0));
    }
    let line_gap = if gaps.is_empty() {
        baseline_font
    } else {
        percentile(gaps.iter().copied(), 0.5).max(f64::MIN_POSITIVE)
    };

    LayoutMetrics { baseline_font, left_margin, line_gap }
}

/// Groups a chapter's flat line/image flow into rendered blocks.
pub fn blocks(events: &[FlowEvent], m: &LayoutMetrics) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    // Text accumulating into the block currently open, and which kind of block that is.
    let mut pending: Option<(Kind, String)> = None;
    let mut last_was_image = false;
    let mut previous_y: Option<f64> = None;

    for event in events {
        let run = match event {
            FlowEvent::Image(idx) => {
                flush(&mut pending, &mut out);
                out.push(Block::Image(*idx));
                last_was_image = true;
                previous_y = None;
                continue;
            }
            FlowEvent::Line(run) => run,
        };
        let text = run.text.trim();
        if text.is_empty() {
            continue;
        }

        let kind = classify(run, text, m, last_was_image);
        let indented = run.x_start > m.left_margin + INDENT_MIN;
        // A gap of zero or less means the page changed, which says nothing about
        // paragraphs — the text may well run straight across the break.
        let gapped = previous_y
            .map(|prev| {
                let gap = run.y - prev;
                gap > 0.0 && gap > m.line_gap * PARAGRAPH_GAP
            })
            .unwrap_or(false);
        let starts_new_paragraph = kind == Kind::Paragraph && (indented || gapped);

        match pending.as_mut() {
            // Same kind of block continuing, and nothing said to break here.
            Some((open, buf)) if *open == kind && !starts_new_paragraph => {
                buf.push(' ');
                buf.push_str(text);
            }
            _ => {
                flush(&mut pending, &mut out);
                pending = Some((kind, text.to_string()));
            }
        }

        last_was_image = false;
        previous_y = Some(run.y);
    }
    flush(&mut pending, &mut out);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Heading,
    Paragraph,
    Caption,
    RightAligned,
}

fn classify(run: &TextRun, text: &str, m: &LayoutMetrics, after_image: bool) -> Kind {
    let chars = text.chars().count();
    // Ornamental separators are frequently set larger than the body text, so size alone
    // isn't enough — a heading has to actually say something.
    let has_words = text.chars().any(|c| c.is_alphanumeric());
    if has_words && run.font_size >= m.baseline_font * HEADING_MIN && chars < HEADING_MAX_CHARS {
        return Kind::Heading;
    }
    if after_image && run.font_size <= m.baseline_font * CAPTION_MAX {
        return Kind::Caption;
    }
    // Attributions, signatures and datelines start deep into the page — far past any
    // paragraph indent, which on a typical page is only a couple of percent wide.
    if run.x_start > RIGHT_ALIGN_MIN_X && chars < RIGHT_ALIGN_MAX_CHARS {
        return Kind::RightAligned;
    }
    Kind::Paragraph
}

fn flush(pending: &mut Option<(Kind, String)>, out: &mut Vec<Block>) {
    let Some((kind, text)) = pending.take() else {
        return;
    };
    out.push(match kind {
        Kind::Heading => Block::Heading(text),
        Kind::Paragraph => Block::Paragraph(text),
        Kind::Caption => Block::Caption(text),
        Kind::RightAligned => Block::RightAligned(text),
    });
}

/// Nearest-rank percentile — all the precision these layout thresholds need.
fn percentile(values: impl Iterator<Item = f64>, q: f64) -> f64 {
    let mut v: Vec<f64> = values.collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((v.len() - 1) as f64 * q).round() as usize;
    v[idx.min(v.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{doc, line, line_at, page_at, BODY_LEFT};

    /// Geometry matching the reference book: margin at 0.126, indents at 0.155, a rock
    /// steady 19.5pt between lines.
    fn book_metrics() -> LayoutMetrics {
        LayoutMetrics { baseline_font: 15.0, left_margin: BODY_LEFT, line_gap: 19.5 }
    }

    const INDENT: f64 = BODY_LEFT + 0.03;

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b {
                Block::Heading(t) => format!("h2:{t}"),
                Block::Paragraph(t) => format!("p:{t}"),
                Block::Caption(t) => format!("legenda:{t}"),
                Block::RightAligned(t) => format!("direita:{t}"),
                Block::Image(i) => format!("img:{i}"),
            })
            .collect()
    }

    /// Successive lines of one paragraph, 19.5pt apart at the left margin.
    fn flowing(items: &[&str], first_y: f64) -> Vec<FlowEvent> {
        items
            .iter()
            .enumerate()
            .map(|(i, t)| line(t, first_y + i as f64 * 19.5, 15.0))
            .collect()
    }

    #[test]
    fn the_baseline_font_is_the_median_line_size() {
        let d = doc(
            vec![page_at(1, vec![line("a", 0.0, 10.0), line("b", 20.0, 10.0), line("c", 40.0, 30.0)])],
            vec![],
        );
        assert_eq!(measure(&d).baseline_font, 10.0);
    }

    #[test]
    fn the_left_margin_is_read_off_the_lines() {
        let d = doc(
            vec![page_at(
                1,
                vec![
                    line("na margem", 0.0, 15.0),
                    line("na margem", 19.5, 15.0),
                    line_at("recuada", 39.0, 15.0, INDENT),
                ],
            )],
            vec![],
        );
        assert!((measure(&d).left_margin - BODY_LEFT).abs() < 0.001);
    }

    #[test]
    fn the_line_gap_is_the_usual_distance_between_lines() {
        let d = doc(vec![page_at(1, flowing(&["a", "b", "c", "d"], 0.0))], vec![]);
        assert_eq!(measure(&d).line_gap, 19.5);
    }

    /// y restarts at the top of every page, so the jump across a page boundary is not a
    /// line gap and must not drag the measurement around.
    #[test]
    fn gaps_across_a_page_boundary_are_not_counted() {
        let d = doc(
            vec![
                page_at(1, flowing(&["a", "b", "c"], 700.0)),
                page_at(2, flowing(&["d", "e", "f"], 60.0)),
            ],
            vec![],
        );
        assert_eq!(measure(&d).line_gap, 19.5);
    }

    #[test]
    fn measuring_an_empty_document_does_not_panic() {
        let m = measure(&doc(vec![], vec![]));
        assert!(m.baseline_font > 0.0 && m.line_gap > 0.0);
    }

    #[test]
    fn lines_of_one_paragraph_are_joined_with_single_spaces() {
        let events = flowing(&["Primeira metade", "da frase."], 0.0);
        assert_eq!(texts(&blocks(&events, &book_metrics())), vec!["p:Primeira metade da frase."]);
    }

    #[test]
    fn an_indented_line_opens_a_new_paragraph() {
        let events = vec![
            line("Fim do primeiro", 0.0, 15.0),
            line_at("Começo do segundo", 19.5, 15.0, INDENT),
            line("continuação dele.", 39.0, 15.0),
        ];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["p:Fim do primeiro", "p:Começo do segundo continuação dele."]
        );
    }

    /// The other convention: no indent, but extra air between paragraphs.
    #[test]
    fn extra_vertical_space_also_opens_a_new_paragraph() {
        let events = vec![
            line("Fim do primeiro", 0.0, 15.0),
            line("Começo do segundo", 19.5 * 2.0, 15.0),
            line("continuação dele.", 19.5 * 3.0, 15.0),
        ];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["p:Fim do primeiro", "p:Começo do segundo continuação dele."]
        );
    }

    /// A paragraph that runs across a page break must not be split just because y jumped
    /// back to the top of the next page.
    #[test]
    fn a_paragraph_continues_across_a_page_break() {
        let events = vec![
            line("Fim da página um", 700.0, 15.0),
            line("topo da página dois", 60.0, 15.0),
        ];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["p:Fim da página um topo da página dois"]
        );
    }

    #[test]
    fn a_bigger_short_line_becomes_a_subheading() {
        let events = vec![
            line("Fim do texto anterior", 0.0, 15.0),
            line("Um subtítulo", 19.5, 20.0),
            line("Texto seguinte.", 39.0, 15.0),
        ];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["p:Fim do texto anterior", "h2:Um subtítulo", "p:Texto seguinte."]
        );
    }

    /// Decorative separators are often set in a larger size than the body, but a run of
    /// ornaments is not a heading — a heading has words in it.
    #[test]
    fn an_ornament_in_a_large_size_is_not_a_heading() {
        let events = vec![line("» » »", 0.0, 20.0)];
        assert_eq!(texts(&blocks(&events, &book_metrics())), vec!["p:» » »"]);
    }

    #[test]
    fn a_subheading_wrapped_over_two_lines_stays_one_heading() {
        let events = vec![line("Um subtítulo que", 0.0, 20.0), line("continua aqui", 19.5, 20.0)];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["h2:Um subtítulo que continua aqui"]
        );
    }

    #[test]
    fn a_smaller_line_right_after_an_image_is_a_caption() {
        let events = vec![
            FlowEvent::Image(3),
            line_at("Figura 5 — vendas por mês", 19.5, 12.0, 0.3),
            line("Texto normal seguindo.", 39.0, 15.0),
        ];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["img:3", "legenda:Figura 5 — vendas por mês", "p:Texto normal seguindo."]
        );
    }

    #[test]
    fn a_smaller_line_far_from_any_image_stays_a_paragraph() {
        let events = vec![line("Uma nota de rodapé qualquer", 0.0, 12.0)];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["p:Uma nota de rodapé qualquer"]
        );
    }

    /// Epigraph attributions ("— Disraeli") sit flush right, far past any paragraph indent.
    /// Rendering them as ordinary paragraphs loses the attribution entirely.
    #[test]
    fn a_line_starting_deep_into_the_page_is_flush_right() {
        let events = vec![
            line("Existem três tipos de mentiras.", 0.0, 15.0),
            line_at("— Disraeli", 19.5, 15.0, 0.62),
        ];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["p:Existem três tipos de mentiras.", "direita:— Disraeli"]
        );
    }

    #[test]
    fn a_right_aligned_line_does_not_absorb_the_paragraph_after_it() {
        let events = vec![
            line_at("— Disraeli", 0.0, 15.0, 0.62),
            line("O pensamento estatístico um dia será", 19.5, 15.0),
        ];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["direita:— Disraeli", "p:O pensamento estatístico um dia será"]
        );
    }

    /// An ordinary paragraph indent is only a few percent of the page wide; it must never
    /// be confused with a line genuinely set flush right.
    #[test]
    fn a_paragraph_indent_is_never_mistaken_for_right_alignment() {
        let events = vec![line_at("Começo de um parágrafo comum", 0.0, 15.0, INDENT)];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["p:Começo de um parágrafo comum"]
        );
    }

    #[test]
    fn an_image_closes_the_paragraph_running_before_it() {
        let events = vec![
            line("Antes.", 0.0, 15.0),
            FlowEvent::Image(0),
            line("Depois.", 39.0, 15.0),
        ];
        assert_eq!(
            texts(&blocks(&events, &book_metrics())),
            vec!["p:Antes.", "img:0", "p:Depois."]
        );
    }

    #[test]
    fn blank_lines_produce_no_block_at_all() {
        let events = vec![line("   ", 0.0, 15.0), line("\t", 19.5, 15.0)];
        assert!(blocks(&events, &book_metrics()).is_empty());
    }
}
