use std::collections::HashMap;
use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use pdf_extract::{MediaBox, OutputDev, OutputError, Transform};

use crate::images;
use crate::pdf_model::{ExtractedDoc, FlowEvent, ImageAsset, PageFlow, TextRun};

/// Parses every page of the PDF at `path` into an ordered flow of text lines and
/// images, in reading order, without ever handing pdf-extract a raw image XObject
/// (which panics trying to parse image bytes as content-stream operators — see
/// docs/plan for the full explanation). Logs progress/warnings via `log`.
pub fn parse_document(path: &Path, mut log: impl FnMut(String)) -> Result<ExtractedDoc, String> {
    let mut doc = Document::load(path).map_err(|e| e.to_string())?;

    let mut page_ids: Vec<(u32, ObjectId)> = doc.get_pages().into_iter().collect();
    page_ids.sort_by_key(|(n, _)| *n);

    let mut images_out: Vec<ImageAsset> = Vec::new();
    let mut pages: Vec<PageFlow> = Vec::new();

    for (page_num, page_id) in page_ids {
        let page_images = images::extract_page_images(&doc, page_id, |m| {
            log(format!("página {page_num}: {m}"))
        });
        let mut id_to_index: HashMap<ObjectId, usize> = HashMap::new();
        for (id, asset) in page_images {
            let idx = images_out.len();
            id_to_index.insert(id, idx);
            images_out.push(asset);
        }

        let (filtered_ops, image_events) =
            match sanitize_and_locate_images(&doc, page_id, &id_to_index) {
                Ok(v) => v,
                Err(e) => {
                    log(format!(
                        "página {page_num}: falha ao ler stream de conteúdo ({e}), texto desta página ficará vazio"
                    ));
                    (Vec::new(), Vec::new())
                }
            };

        if let Err(e) = patch_page_contents(&mut doc, page_id, &filtered_ops) {
            log(format!("página {page_num}: falha ao higienizar conteúdo ({e})"));
        }

        let mut recorder = FlowRecorder::default();
        if let Err(e) = pdf_extract::output_doc_page(&doc, &mut recorder, page_num) {
            log(format!(
                "página {page_num}: pdf-extract falhou ({e}), texto desta página ficará vazio"
            ));
        }

        let mut events: Vec<(f64, FlowEvent)> = recorder
            .lines
            .into_iter()
            .map(|run| (run.y, FlowEvent::Line(run)))
            .collect();
        events.extend(
            image_events
                .into_iter()
                .map(|(y, idx)| (y, FlowEvent::Image(idx))),
        );
        events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        log(format!(
            "página {page_num}: {} linhas, {} imagens",
            events.iter().filter(|(_, e)| matches!(e, FlowEvent::Line(_))).count(),
            events.iter().filter(|(_, e)| matches!(e, FlowEvent::Image(_))).count(),
        ));

        pages.push(PageFlow { page_num, events });
    }

    Ok(ExtractedDoc {
        pages,
        images: images_out,
    })
}

fn as_num(o: &Object) -> f64 {
    o.as_float()
        .map(|f| f as f64)
        .or_else(|_| o.as_i64().map(|i| i as f64))
        .unwrap_or(0.0)
}

fn get_resources<'a>(doc: &'a Document, page_id: ObjectId) -> Result<Option<&'a Dictionary>, String> {
    let mut current = doc.get_dictionary(page_id).map_err(|e| e.to_string())?;
    loop {
        if let Ok(r) = current.get(b"Resources") {
            if let Ok((_, obj)) = doc.dereference(r) {
                if let Ok(d) = obj.as_dict() {
                    return Ok(Some(d));
                }
            }
        }
        match current.get(b"Parent").and_then(|p| p.as_reference()) {
            Ok(parent_id) => current = doc.get_dictionary(parent_id).map_err(|e| e.to_string())?,
            Err(_) => return Ok(None),
        }
    }
}

fn get_media_box(doc: &Document, page_id: ObjectId) -> Result<(f64, f64, f64, f64), String> {
    let mut current = doc.get_dictionary(page_id).map_err(|e| e.to_string())?;
    loop {
        if let Ok(mb) = current.get(b"MediaBox") {
            if let Ok((_, obj)) = doc.dereference(mb) {
                if let Ok(arr) = obj.as_array() {
                    if arr.len() == 4 {
                        return Ok((as_num(&arr[0]), as_num(&arr[1]), as_num(&arr[2]), as_num(&arr[3])));
                    }
                }
            }
        }
        match current.get(b"Parent").and_then(|p| p.as_reference()) {
            Ok(parent_id) => current = doc.get_dictionary(parent_id).map_err(|e| e.to_string())?,
            Err(_) => return Err("MediaBox não encontrado".into()),
        }
    }
}

/// Walks the page's content stream once, removing every `Do` that targets an Image
/// XObject (recording its on-page position instead) while leaving every other
/// operation — including `Do`s that target Form XObjects, which are safe for
/// pdf-extract to recurse into — untouched.
fn sanitize_and_locate_images(
    doc: &Document,
    page_id: ObjectId,
    id_to_index: &HashMap<ObjectId, usize>,
) -> Result<(Vec<Operation>, Vec<(f64, usize)>), String> {
    let content = doc.get_and_decode_page_content(page_id).map_err(|e| e.to_string())?;
    let resources = get_resources(doc, page_id)?;
    let xobjects = resources.and_then(|r| doc.get_dict_in_dict(r, b"XObject").ok());
    let (llx, lly, _urx, ury) = get_media_box(doc, page_id)?;
    let flip: Transform = Transform::row_major(1., 0., 0., -1., 0., ury - lly);
    let _ = llx;

    let mut filtered = Vec::with_capacity(content.operations.len());
    let mut image_events = Vec::new();
    let mut ctm_stack: Vec<Transform> = Vec::new();
    let mut ctm = Transform::identity();

    for op in content.operations {
        match op.operator.as_str() {
            "q" => {
                ctm_stack.push(ctm);
                filtered.push(op);
            }
            "Q" => {
                if let Some(m) = ctm_stack.pop() {
                    ctm = m;
                }
                filtered.push(op);
            }
            "cm" if op.operands.len() == 6 => {
                let m = Transform::row_major(
                    as_num(&op.operands[0]),
                    as_num(&op.operands[1]),
                    as_num(&op.operands[2]),
                    as_num(&op.operands[3]),
                    as_num(&op.operands[4]),
                    as_num(&op.operands[5]),
                );
                ctm = ctm.pre_transform(&m);
                filtered.push(op);
            }
            "Do" => {
                let target = op
                    .operands
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .and_then(|name| xobjects.and_then(|x| x.get(name).ok()))
                    .and_then(|obj| obj.as_reference().ok())
                    .and_then(|id| id_to_index.get(&id).copied());
                if let Some(idx) = target {
                    let p0 = ctm.transform_point(euclid::Point2D::new(0.0_f64, 0.0));
                    let p1 = ctm.transform_point(euclid::Point2D::new(0.0_f64, 1.0));
                    let p0f = flip.transform_point(p0);
                    let p1f = flip.transform_point(p1);
                    let y = p0f.y.min(p1f.y);
                    image_events.push((y, idx));
                } else {
                    filtered.push(op);
                }
            }
            _ => filtered.push(op),
        }
    }
    Ok((filtered, image_events))
}

fn patch_page_contents(doc: &mut Document, page_id: ObjectId, ops: &[Operation]) -> Result<(), String> {
    let content = Content {
        operations: ops.to_vec(),
    };
    let bytes = content.encode().map_err(|e| e.to_string())?;
    let stream_id = doc.add_object(Stream::new(Dictionary::new(), bytes));
    let page_dict = doc
        .get_object_mut(page_id)
        .map_err(|e| e.to_string())?
        .as_dict_mut()
        .map_err(|e| e.to_string())?;
    page_dict.set("Contents", Object::Reference(stream_id));
    Ok(())
}

pub fn info_metadata(doc: &Document) -> (Option<String>, Option<String>) {
    let info_dict = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|obj| doc.dereference(obj).ok())
        .and_then(|(_, obj)| obj.as_dict().ok());
    let get_str = |key: &[u8]| -> Option<String> {
        let dict = info_dict?;
        let obj = dict.get(key).ok()?;
        match obj {
            Object::String(bytes, _) => {
                let s = decode_pdf_string(bytes);
                if s.trim().is_empty() { None } else { Some(s.trim().to_string()) }
            }
            _ => None,
        }
    };
    (get_str(b"Title"), get_str(b"Author"))
}

fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Custom `OutputDev`: reconstructs text lines (with y-position and font size) using
/// the same y-jump / x-gap heuristics as pdf-extract's own `PlainTextOutput`, but keeps
/// each line as a discrete structured record instead of a single flat string.
#[derive(Default)]
struct FlowRecorder {
    flip_ctm: Transform,
    page_width: f64,
    lines: Vec<TextRun>,
    cur_text: String,
    cur_y: f64,
    cur_font_size: f64,
    cur_x_start: f64,
    last_end: f64,
    last_y: f64,
    line_start: bool,
    first_char_of_word: bool,
    any_output_yet: bool,
}

impl FlowRecorder {
    fn flush_line(&mut self) {
        let text = self.cur_text.trim();
        if !text.is_empty() {
            let width = if self.page_width > 0.0 { self.page_width } else { 1.0 };
            self.lines.push(TextRun {
                text: text.to_string(),
                y: self.cur_y,
                font_size: if self.cur_font_size > 0.0 { self.cur_font_size } else { 1.0 },
                x_start: (self.cur_x_start / width).clamp(0.0, 1.0),
            });
        }
        self.cur_text.clear();
        self.cur_font_size = 0.0;
        self.cur_x_start = f64::MAX;
        self.line_start = true;
    }
}

impl OutputDev for FlowRecorder {
    fn begin_page(&mut self, _page_num: u32, media_box: &MediaBox, _art_box: Option<(f64, f64, f64, f64)>) -> Result<(), OutputError> {
        self.flip_ctm = Transform::row_major(1., 0., 0., -1., 0., media_box.ury - media_box.lly);
        self.page_width = media_box.urx - media_box.llx;
        self.cur_x_start = f64::MAX;
        self.last_end = 100000.;
        self.last_y = 0.;
        self.line_start = true;
        Ok(())
    }

    fn end_page(&mut self) -> Result<(), OutputError> {
        self.flush_line();
        Ok(())
    }

    fn output_character(&mut self, trm: &Transform, width: f64, _spacing: f64, font_size: f64, char: &str) -> Result<(), OutputError> {
        let position = trm.post_transform(&self.flip_ctm);
        let transformed_font_size_vec = trm.transform_vector(euclid::vec2(font_size, font_size));
        let transformed_font_size = (transformed_font_size_vec.x * transformed_font_size_vec.y)
            .abs()
            .sqrt();
        let (x, y) = (position.m31, position.m32);

        if self.first_char_of_word && self.any_output_yet {
            // Only break the line here; do NOT synthesize inter-word spaces from the x-gap.
            // Real-world justified PDFs (this book included) reliably embed an actual space
            // glyph for every real word boundary, and often spread justification stretch
            // across individual letter-kerning pairs too — a gap-based heuristic can't tell
            // those apart and ends up inserting spaces *inside* words instead. Trusting only
            // the glyphs the PDF actually emits (plus real space characters) reads far
            // cleaner in practice than a geometric guess.
            let big_y_jump = (y - self.last_y).abs() > transformed_font_size * 1.5;
            let moved_left_down = x < self.last_end && (y - self.last_y).abs() > transformed_font_size * 0.5;
            if big_y_jump || moved_left_down {
                self.flush_line();
            }
        }
        self.first_char_of_word = false;

        if self.line_start {
            self.cur_y = y;
            self.line_start = false;
        }
        self.cur_text.push_str(char);
        self.cur_font_size = self.cur_font_size.max(transformed_font_size);
        self.cur_x_start = self.cur_x_start.min(x);
        self.any_output_yet = true;
        self.last_y = y;
        self.last_end = x + width * transformed_font_size;
        Ok(())
    }

    fn begin_word(&mut self) -> Result<(), OutputError> {
        self.first_char_of_word = true;
        Ok(())
    }

    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::StringFormat;

    fn doc_with_info(entries: &[(&str, Object)]) -> Document {
        let mut doc = Document::with_version("1.5");
        let mut info = Dictionary::new();
        for (key, value) in entries {
            info.set(*key, value.clone());
        }
        let id = doc.add_object(Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(id));
        doc
    }

    fn literal(bytes: &[u8]) -> Object {
        Object::String(bytes.to_vec(), StringFormat::Literal)
    }

    /// PDF text strings are UTF-16BE when they carry a byte-order mark, and an 8-bit
    /// encoding otherwise — getting this backwards is what turns "Coração" into mojibake.
    #[test]
    fn utf16_strings_are_decoded_through_their_byte_order_mark() {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in "Coração".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_pdf_string(&bytes), "Coração");
    }

    #[test]
    fn strings_without_a_bom_are_read_as_latin1() {
        assert_eq!(decode_pdf_string(&[0x43, 0x6F, 0x72, 0x61, 0xE7, 0xE3, 0x6F]), "Coração");
    }

    #[test]
    fn a_lone_bom_decodes_to_nothing_rather_than_panicking() {
        assert_eq!(decode_pdf_string(&[0xFE, 0xFF]), "");
    }

    #[test]
    fn an_odd_trailing_byte_after_the_bom_is_ignored() {
        let bytes = vec![0xFE, 0xFF, 0x00, 0x41, 0x00];
        assert_eq!(decode_pdf_string(&bytes), "A");
    }

    #[test]
    fn title_and_author_come_from_the_info_dictionary() {
        let doc = doc_with_info(&[("Title", literal(b"Meu Livro")), ("Author", literal(b"Fulano"))]);
        assert_eq!(
            info_metadata(&doc),
            (Some("Meu Livro".to_string()), Some("Fulano".to_string()))
        );
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_off_the_metadata() {
        let doc = doc_with_info(&[("Title", literal(b"  Meu Livro  "))]);
        assert_eq!(info_metadata(&doc).0, Some("Meu Livro".to_string()));
    }

    #[test]
    fn a_blank_title_counts_as_absent_so_the_filename_can_take_over() {
        let doc = doc_with_info(&[("Title", literal(b"   "))]);
        assert_eq!(info_metadata(&doc), (None, None));
    }

    #[test]
    fn a_non_string_title_is_ignored() {
        let doc = doc_with_info(&[("Title", Object::Integer(42))]);
        assert_eq!(info_metadata(&doc).0, None);
    }

    #[test]
    fn a_document_with_no_info_dictionary_reports_nothing() {
        assert_eq!(info_metadata(&Document::with_version("1.5")), (None, None));
    }

    #[test]
    fn numbers_are_read_from_either_pdf_representation() {
        assert_eq!(as_num(&Object::Integer(72)), 72.0);
        assert_eq!(as_num(&Object::Real(72.5)), 72.5);
        assert_eq!(as_num(&Object::Null), 0.0, "valor não numérico vira zero");
    }
}
