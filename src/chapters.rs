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
                    let is_size_heading = has_letter_or_digit
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
