#[derive(Debug, Clone)]
pub struct TextRun {
    pub text: String,
    pub y: f64,
    pub font_size: f64,
}

#[derive(Debug, Clone)]
pub struct ImageAsset {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub mime: &'static str,
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
