use regex::Regex;

use crate::pdf_model::{ExtractedDoc, FlowEvent};

pub struct Chapter {
    pub title: String,
    pub events: Vec<FlowEvent>,
}

const HEADING_PATTERNS: &[&str] = &[
    r"(?i)^\s*cap[ií]tulo\s+\d+",
    r"(?i)^\s*chapter\s+\d+",
    r"(?i)^\s*parte\s+\d+",
    r"(?i)^\s*part\s+\d+",
];

/// Splits the document's flat text/image flow into chapters. A line is treated as a
/// chapter boundary if it matches a common "Chapter N"/"Capítulo N" pattern, or if its
/// font size is well above the document's body-text baseline and it reads like a short
/// title rather than a paragraph. This is a heuristic — every detected heading is logged
/// so it can be checked against the real book instead of trusted blindly.
pub fn detect_chapters(doc: &ExtractedDoc, mut log: impl FnMut(String)) -> Vec<Chapter> {
    let baseline = body_font_baseline(doc);
    let heading_regexes: Vec<Regex> = HEADING_PATTERNS
        .iter()
        .map(|p| Regex::new(p).expect("padrão de capítulo inválido"))
        .collect();

    let mut chapters: Vec<Chapter> = Vec::new();
    let mut current_title = "Introdução".to_string();
    let mut current_events: Vec<FlowEvent> = Vec::new();
    // Tracks whether the immediately preceding line was itself a detected heading, with
    // no body text since — consecutive heading lines are almost always one wrapped title
    // (e.g. "CAPÍTULO 1" / "A amostra com" / "tendenciosidade embutida") and must be
    // merged into a single chapter rather than split into three empty chapters.
    let mut in_heading_run = false;

    for page in &doc.pages {
        for (_, event) in &page.events {
            match event {
                FlowEvent::Line(run) => {
                    let text = run.text.trim();
                    let has_letter_or_digit = text.chars().any(|c| c.is_alphanumeric());
                    let is_regex_heading = has_letter_or_digit && heading_regexes.iter().any(|r| r.is_match(text));
                    // `baseline > 0.0` matters: some PDFs report no usable font size at
                    // all, and without this every single line would clear the threshold.
                    let is_size_heading = has_letter_or_digit
                        && baseline > 0.0
                        && run.font_size >= baseline * 1.25
                        && text.chars().count() < 80;
                    if is_regex_heading || is_size_heading {
                        if in_heading_run {
                            // Continuation of the same (wrapped) heading: append, don't split.
                            current_title.push(' ');
                            current_title.push_str(text);
                        } else {
                            if !current_events.is_empty() {
                                chapters.push(Chapter {
                                    title: current_title.clone(),
                                    events: std::mem::take(&mut current_events),
                                });
                            }
                            current_title = text.to_string();
                        }
                        log(format!(
                            "capítulo detectado (pág. {}): \"{}\" (fonte {:.1}pt, baseline {:.1}pt)",
                            page.page_num, text, run.font_size, baseline
                        ));
                        in_heading_run = true;
                        continue;
                    }
                    in_heading_run = false;
                    current_events.push(FlowEvent::Line(run.clone()));
                }
                FlowEvent::Image(idx) => {
                    in_heading_run = false;
                    current_events.push(FlowEvent::Image(*idx));
                }
            }
        }
    }
    if !current_events.is_empty() || chapters.is_empty() {
        chapters.push(Chapter {
            title: current_title,
            events: current_events,
        });
    }
    chapters
}

fn body_font_baseline(doc: &ExtractedDoc) -> f64 {
    let mut sizes: Vec<f64> = doc
        .pages
        .iter()
        .flat_map(|p| p.events.iter())
        .filter_map(|(_, e)| match e {
            FlowEvent::Line(run) => Some(run.font_size),
            FlowEvent::Image(_) => None,
        })
        .collect();
    if sizes.is_empty() {
        return 1.0;
    }
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sizes[sizes.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{doc, line, page};

    fn titles(chapters: &[Chapter]) -> Vec<&str> {
        chapters.iter().map(|c| c.title.as_str()).collect()
    }

    fn texts(chapter: &Chapter) -> Vec<&str> {
        chapter
            .events
            .iter()
            .filter_map(|e| match e {
                FlowEvent::Line(run) => Some(run.text.as_str()),
                FlowEvent::Image(_) => None,
            })
            .collect()
    }

    #[test]
    fn text_with_no_heading_becomes_a_single_introduction() {
        let d = doc(
            vec![page(1, vec![line("Um parágrafo.", 0.0, 10.0), line("Outro.", 10.0, 10.0)])],
            vec![],
        );
        let chapters = detect_chapters(&d, |_| {});

        assert_eq!(titles(&chapters), vec!["Introdução"]);
        assert_eq!(texts(&chapters[0]), vec!["Um parágrafo.", "Outro."]);
    }

    #[test]
    fn a_chapter_pattern_starts_a_new_chapter() {
        let d = doc(
            vec![page(
                1,
                vec![
                    line("Abertura.", 0.0, 10.0),
                    line("Capítulo 1", 10.0, 10.0),
                    line("Corpo do capítulo.", 20.0, 10.0),
                ],
            )],
            vec![],
        );
        let chapters = detect_chapters(&d, |_| {});

        assert_eq!(titles(&chapters), vec!["Introdução", "Capítulo 1"]);
        assert_eq!(texts(&chapters[0]), vec!["Abertura."]);
        assert_eq!(texts(&chapters[1]), vec!["Corpo do capítulo."]);
    }

    #[test]
    fn the_chapter_pattern_ignores_case_and_language() {
        for heading in ["CAPÍTULO 2", "capitulo 3", "Chapter 4", "PARTE 5", "part 6"] {
            let d = doc(
                vec![page(
                    1,
                    vec![
                        line("Antes.", 0.0, 10.0),
                        line(heading, 10.0, 10.0),
                        line("Depois.", 20.0, 10.0),
                    ],
                )],
                vec![],
            );
            let chapters = detect_chapters(&d, |_| {});
            assert_eq!(titles(&chapters), vec!["Introdução", heading], "falhou em {heading}");
        }
    }

    #[test]
    fn a_jump_in_font_size_also_starts_a_chapter() {
        let d = doc(
            vec![page(
                1,
                vec![
                    line("Corpo.", 0.0, 10.0),
                    line("Um Título Sem Padrão", 10.0, 20.0),
                    line("Mais corpo.", 20.0, 10.0),
                ],
            )],
            vec![],
        );
        assert_eq!(
            titles(&detect_chapters(&d, |_| {})),
            vec!["Introdução", "Um Título Sem Padrão"]
        );
    }

    #[test]
    fn a_long_line_in_a_big_font_is_a_paragraph_not_a_title() {
        let long = "a".repeat(120);
        let d = doc(
            vec![page(1, vec![line("Corpo.", 0.0, 10.0), line(&long, 10.0, 20.0)])],
            vec![],
        );
        assert_eq!(titles(&detect_chapters(&d, |_| {})), vec!["Introdução"]);
    }

    #[test]
    fn a_wrapped_title_is_merged_into_one_chapter() {
        let d = doc(
            vec![page(
                1,
                vec![
                    line("Corpo antes.", 0.0, 10.0),
                    line("Mais corpo antes.", 10.0, 10.0),
                    line("CAPÍTULO 1", 20.0, 20.0),
                    line("A amostra com", 30.0, 20.0),
                    line("tendenciosidade embutida", 40.0, 20.0),
                    line("Texto do capítulo.", 50.0, 10.0),
                    line("Mais texto do capítulo.", 60.0, 10.0),
                ],
            )],
            vec![],
        );
        let chapters = detect_chapters(&d, |_| {});

        assert_eq!(
            titles(&chapters),
            vec!["Introdução", "CAPÍTULO 1 A amostra com tendenciosidade embutida"]
        );
        assert_eq!(
            texts(&chapters[1]),
            vec!["Texto do capítulo.", "Mais texto do capítulo."]
        );
    }

    #[test]
    fn images_stay_attached_to_the_chapter_they_appear_in() {
        let d = doc(
            vec![page(
                1,
                vec![
                    line("Capítulo 1", 0.0, 10.0),
                    FlowEvent::Image(0),
                    line("Capítulo 2", 20.0, 10.0),
                    FlowEvent::Image(1),
                ],
            )],
            vec![],
        );
        let chapters = detect_chapters(&d, |_| {});

        assert_eq!(chapters[0].title, "Capítulo 1");
        assert!(matches!(chapters[0].events.as_slice(), [FlowEvent::Image(0)]));
    }

    #[test]
    fn chapters_span_page_boundaries() {
        let d = doc(
            vec![
                page(1, vec![line("Capítulo 1", 0.0, 10.0), line("Começo.", 10.0, 10.0)]),
                page(2, vec![line("Continuação.", 0.0, 10.0)]),
            ],
            vec![],
        );
        let chapters = detect_chapters(&d, |_| {});

        assert_eq!(titles(&chapters), vec!["Capítulo 1"]);
        assert_eq!(texts(&chapters[0]), vec!["Começo.", "Continuação."]);
    }

    /// A heading with nothing under it would show up in the ToC as a dead link, so the
    /// last chapter is only kept when it actually has content.
    #[test]
    fn a_trailing_heading_with_no_body_is_dropped() {
        let d = doc(
            vec![page(1, vec![line("Corpo.", 0.0, 10.0), line("Capítulo 9", 10.0, 10.0)])],
            vec![],
        );
        assert_eq!(titles(&detect_chapters(&d, |_| {})), vec!["Introdução"]);
    }

    #[test]
    fn an_empty_document_yields_one_empty_chapter_instead_of_panicking() {
        let chapters = detect_chapters(&doc(vec![], vec![]), |_| {});
        assert_eq!(titles(&chapters), vec!["Introdução"]);
        assert!(chapters[0].events.is_empty());
    }

    #[test]
    fn every_detected_heading_is_logged_for_review() {
        let d = doc(
            vec![page(1, vec![line("Corpo.", 0.0, 10.0), line("Capítulo 1", 10.0, 10.0)])],
            vec![],
        );
        let mut logs = Vec::new();
        detect_chapters(&d, |s| logs.push(s));

        assert_eq!(logs.len(), 1, "{logs:?}");
        assert!(logs[0].contains("Capítulo 1"), "{logs:?}");
    }

    /// Some PDFs report no usable font size at all. The size heuristic has nothing to go
    /// on there, so it must stay quiet instead of turning every single line into a title.
    #[test]
    fn a_document_with_no_font_sizes_produces_no_size_headings() {
        let d = doc(
            vec![page(
                1,
                vec![
                    line("Primeira linha.", 0.0, 0.0),
                    line("Segunda linha.", 10.0, 0.0),
                    line("Terceira linha.", 20.0, 0.0),
                ],
            )],
            vec![],
        );
        let chapters = detect_chapters(&d, |_| {});

        assert_eq!(titles(&chapters), vec!["Introdução"]);
        assert_eq!(chapters[0].events.len(), 3, "nenhuma linha deveria virar título");
    }

    #[test]
    fn the_baseline_is_the_median_body_size() {
        let d = doc(
            vec![page(
                1,
                vec![
                    line("a", 0.0, 10.0),
                    line("b", 10.0, 10.0),
                    line("c", 20.0, 10.0),
                    line("d", 30.0, 40.0),
                ],
            )],
            vec![],
        );
        // Com mediana 10, o 40pt destoa e vira título; se a média fosse usada (17.5),
        // o limiar subiria e o título passaria batido.
        assert_eq!(body_font_baseline(&d), 10.0);
    }
}
