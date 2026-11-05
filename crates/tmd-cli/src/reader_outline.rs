//! Transient top-level heading selection; document position stays in reader.
use tmd_core::{Heading, Line, LineSize, Span, Style, truncate};

#[derive(Default)]
pub(crate) struct Outline {
    pub open: bool,
    pub selected: usize,
    pub notice: String,
}

impl Outline {
    pub fn show(&mut self, headings: &[Heading], source_start: usize) {
        self.notice.clear();
        self.open = !headings.is_empty();
        if !self.open {
            self.notice = " · 未找到顶层标题".into();
            return;
        }
        self.selected = headings
            .iter()
            .rposition(|heading| heading.source.start <= source_start)
            .unwrap_or(0);
    }

    pub fn rows(&self, headings: &[Heading], width: u16) -> Vec<Line> {
        headings
            .iter()
            .map(|heading| {
                let title = if heading.title.is_empty() {
                    "（空标题）"
                } else {
                    &heading.title
                };
                let text = format!(
                    "{}H{} {} · L{}",
                    "  ".repeat(heading.level.saturating_sub(1) as usize),
                    heading.level,
                    title,
                    heading.source_line
                );
                Line {
                    spans: vec![Span::new(
                        truncate(&text, width.saturating_sub(1) as usize),
                        Style::default(),
                    )],
                    src: 0..0,
                    size: LineSize::Normal,
                }
            })
            .collect()
    }

    pub fn top(&self, height: usize) -> usize {
        self.selected.saturating_sub(height.saturating_sub(1))
    }

    pub fn move_by(&mut self, amount: isize, count: usize) {
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(count.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmd_core::{Document, Theme};

    #[test]
    fn opens_at_current_section_and_keeps_duplicate_entries() {
        let doc = Document::new(
            "preamble\n\n# Same\n\nbody\n\n## Same\n\n###\n",
            Theme::default(),
        );
        let headings = doc.headings();
        let mut outline = Outline::default();
        outline.show(headings, 0);
        assert!(outline.open);
        assert_eq!(outline.selected, 0);
        outline.show(headings, headings[1].source.start + 2);
        assert_eq!(outline.selected, 1);
        outline.move_by(isize::MAX, headings.len());
        assert_eq!(outline.selected, 2);
        let rows = outline.rows(headings, 80);
        assert!(rows[2].plain_text().contains("（空标题）"));
        assert_eq!(rows.len(), 3);
        for width in [0, 1, 4, 12] {
            assert!(
                outline
                    .rows(headings, width)
                    .iter()
                    .all(|row| row.width() <= width as usize)
            );
        }
    }

    #[test]
    fn empty_outline_stays_in_reading() {
        let doc = Document::new("> # example\n\n```md\n# example\n```", Theme::default());
        let mut outline = Outline::default();
        outline.show(doc.headings(), 0);
        assert!(!outline.open);
        assert!(outline.notice.contains("未找到"));
    }
}
