#[derive(Debug, Clone)]
pub struct TextRun {
    pub text: String,
    pub y: f64,
    pub font_size: f64,
    /// Where the line starts, as a fraction of the page width. Normalising here lets
    /// lines from differently sized pages be compared directly when inferring the margin,
    /// paragraph indents and right alignment.
    ///
    /// There is deliberately no `x_end`: pdf-extract reports a zero glyph advance for
    /// almost every character, so where a line *ends* cannot be recovered. See `layout`.
    pub x_start: f64,
}

#[derive(Debug, Clone)]
pub struct ImageAsset {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    /// Pixel dimensions as declared by the image XObject; used to pick a cover
    /// candidate and to size the cover page's SVG viewport.
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub enum FlowEvent {
    Line(TextRun),
    Image(usize),
}

#[derive(Debug, Clone)]
pub struct PageFlow {
    pub page_num: u32,
    /// (y, event) sorted ascending by y (top of page first).
    pub events: Vec<(f64, FlowEvent)>,
}

pub struct ExtractedDoc {
    pub pages: Vec<PageFlow>,
    pub images: Vec<ImageAsset>,
}
