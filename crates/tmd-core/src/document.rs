//! Static document text and its one-way mapping into laid-out spans.
use crate::{
    Line, RenderOptions, Span, Theme,
    parse::{self, Block, TopBlock},
    render_blocks,
};
use std::ops::Range;

pub struct Document {
    blocks: Vec<TopBlock>,
    text: String,
    theme: Theme,
    code_blocks: Vec<CodeSnippet>,
    headings: Vec<Heading>,
}

impl Document {
    pub fn new(markdown: &str, theme: Theme) -> Self {
        let mut blocks = parse::parse_for_reader(markdown, &theme);
        let mut text = String::new();
        for block in &mut blocks {
            index_block(&mut block.block, &mut text);
        }
        let newline_offsets: Vec<usize> = markdown.match_indices('\n').map(|(i, _)| i).collect();
        let headings = blocks
            .iter()
            .filter_map(|top| match &top.block {
                Block::Heading { level, spans } => {
                    let title: String = spans.iter().map(|span| span.text.as_str()).collect();
                    Some(Heading {
                        level: *level,
                        title: title.split_whitespace().collect::<Vec<_>>().join(" "),
                        source_line: newline_offsets
                            .partition_point(|&offset| offset < top.src.start)
                            + 1,
                        source: top.src.clone(),
                    })
                }
                _ => None,
            })
            .collect();
        let mut code_blocks = Vec::new();
        for block in &mut blocks {
            collect_codes(&mut block.block, &newline_offsets, &mut code_blocks);
        }
        Self {
            headings,
            code_blocks,
            blocks,
            text,
            theme,
        }
    }

    pub fn code_blocks(&self) -> &[CodeSnippet] {
        &self.code_blocks
    }

    /// Top-level ATX/Setext headings in source order, including duplicates.
    pub fn headings(&self) -> &[Heading] {
        &self.headings
    }

    /// A top-level heading's first layout row, excluding its separating blank.
    /// Exact source ranges also locate empty titles and repeated heading text.
    pub fn heading_line(lines: &[Line], heading: &Heading) -> Option<usize> {
        let first = lines.iter().position(|line| line.src == heading.source)?;
        let separator = first > 0
            && lines[first].spans.is_empty()
            && lines
                .get(first + 1)
                .is_some_and(|line| line.src == heading.source);
        Some(first + usize::from(separator))
    }

    pub fn layout(&self, width: u16, options: RenderOptions) -> Vec<Line> {
        render_blocks(&self.blocks, width, &self.theme, options)
    }

    /// Literal, case-sensitive matches in semantic text, independent of wrapping.
    pub fn find(&self, query: &str) -> Vec<Range<usize>> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        self.text
            .match_indices(query)
            .map(|(i, value)| i..i + value.len())
            .collect()
    }

    /// First logical content line representing a match. Clipped table content
    /// maps to that row's ellipsis rather than to a different source block.
    pub fn match_line(lines: &[Line], range: &Range<usize>) -> Option<usize> {
        lines.iter().position(|line| {
            line.spans.iter().any(|span| {
                span.text_ranges
                    .iter()
                    .any(|r| r.start < range.end && r.end > range.start)
            })
        })
    }
}

/// A document heading, independent of display wrapping or terminal size.
#[derive(Debug)]
pub struct Heading {
    pub level: u8,
    pub title: String,
    pub source_line: usize,
    pub source: Range<usize>,
}

/// A parser-recognized code block, independent of terminal display wrapping.
#[derive(Debug)]
pub struct CodeSnippet {
    pub language: Option<String>,
    pub source_line: usize,
    pub source: Range<usize>,
    /// Code content: no Markdown fences/container markers; CRLF becomes LF.
    pub text: String,
}

fn collect_codes(block: &mut Block, newlines: &[usize], out: &mut Vec<CodeSnippet>) {
    match block {
        Block::Code(code) => {
            code.index = Some(out.len());
            out.push(CodeSnippet {
                language: code.lang.clone(),
                source_line: newlines.partition_point(|&offset| offset < code.src.start) + 1,
                source: code.src.clone(),
                text: code.copy_text.take().unwrap_or_default(),
            });
        }
        Block::List { items, .. } => {
            for item in items {
                for block in &mut item.blocks {
                    collect_codes(block, newlines, out);
                }
            }
        }
        Block::Quote(blocks) => {
            for block in blocks {
                collect_codes(block, newlines, out);
            }
        }
        _ => {}
    }
}

fn index_spans(spans: &mut [Span], text: &mut String) {
    text.push('\n');
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        span.text_ranges = std::iter::once(start..text.len()).collect();
    }
}

fn index_block(block: &mut Block, text: &mut String) {
    match block {
        Block::Paragraph(spans) | Block::Heading { spans, .. } => index_spans(spans, text),
        Block::Code(code) => {
            for line in &mut code.lines {
                text.push('\n');
                let start = text.len();
                text.push_str(&line.text);
                line.text_range = Some(start..text.len());
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for block in &mut item.blocks {
                    index_block(block, text);
                }
            }
        }
        Block::Quote(blocks) => {
            for block in blocks {
                index_block(block, text);
            }
        }
        Block::Table(table) => {
            for cell in &mut table.header {
                index_spans(cell, text);
            }
            for row in &mut table.rows {
                for cell in row {
                    index_spans(cell, text);
                }
            }
        }
        Block::Rule => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_keeps_top_level_atx_setext_and_duplicate_semantic_titles() {
        let md = "# **Top** [label](https://example.test)\n\nSetext\n------\n\n### Same\n\n### Same\n\n####\n\n##### Fifth\n\n###### Sixth\n\n> # quoted example\n\n- # list example\n\n```md\n# fenced example\n```\n";
        let doc = Document::new(md, Theme::default());
        let headings = doc.headings();
        assert_eq!(
            headings.iter().map(|h| h.level).collect::<Vec<_>>(),
            [1, 2, 3, 3, 4, 5, 6]
        );
        assert_eq!(
            headings
                .iter()
                .map(|h| h.title.as_str())
                .collect::<Vec<_>>(),
            ["Top label", "Setext", "Same", "Same", "", "Fifth", "Sixth"]
        );
        assert_eq!(headings[0].source_line, 1);
        assert_eq!(headings[1].source_line, 3);
        assert_eq!(headings[2].source_line, 6);
        assert_ne!(headings[2].source, headings[3].source);
        assert!(doc.find("quoted example").len() == 1);
    }

    #[test]
    fn outline_jumps_skip_separators_and_survive_reflow_and_large_headings() {
        let md = "intro\n\n# Same\n\nbody\n\n# Same\n\n###\n\nlast";
        let doc = Document::new(md, Theme::default());
        let normal = doc.layout(80, RenderOptions::default());
        assert_eq!(
            doc.headings()
                .iter()
                .map(|h| Document::heading_line(&normal, h).unwrap())
                .collect::<Vec<_>>(),
            [2, 7, 10]
        );
        for width in [1, 4, 12, 80] {
            for large in [false, true] {
                let lines = doc.layout(
                    width,
                    RenderOptions {
                        large_headings: large,
                    },
                );
                let mut previous = 0;
                for heading in doc.headings() {
                    let row = Document::heading_line(&lines, heading).unwrap();
                    assert!(row > previous);
                    assert_eq!(lines[row].src, heading.source);
                    assert!(lines[row - 1].spans.is_empty());
                    assert!(lines[row].width() <= width as usize);
                    previous = row;
                }
            }
        }
        let doc = Document::new("###\n", Theme::default());
        assert_eq!(
            Document::heading_line(
                &doc.layout(80, RenderOptions::default()),
                &doc.headings()[0]
            ),
            Some(0)
        );
    }

    #[test]
    fn outline_titles_are_single_line_and_no_headings_is_valid() {
        let doc = Document::new("First<br>second\n==========\n", Theme::default());
        assert_eq!(doc.headings()[0].title, "First second");
        let doc = Document::new("plain\n\n> # nested\n", Theme::default());
        assert!(doc.headings().is_empty());
    }

    #[test]
    fn code_content_preserves_tabs_spaces_newlines_and_normalizes_crlf() {
        let md = "```text\r\n\t中文  \r\n\r\nlast\r\n```\r\n\n    indented\n      extra  \n\n> ```sh\n> echo 中文\n> ```\n\n- item\n\n  ```rs\n  let x = 1;\n  ```\n\n```\nunclosed";
        let doc = Document::new(md, Theme::default());
        let codes = doc.code_blocks();
        assert_eq!(codes.len(), 5);
        assert_eq!(codes[0].text, "\t中文  \n\nlast\n");
        assert_eq!(codes[1].text, "indented\n  extra  \n");
        assert_eq!(codes[2].text, "echo 中文\n");
        assert_eq!(codes[3].text, "let x = 1;\n");
        assert_eq!(codes[4].text, "unclosed");
        assert_eq!(codes[0].source_line, 1);
        assert_eq!(codes[2].source_line, 10);
        for code in codes {
            assert!(md.is_char_boundary(code.source.start));
        }
    }

    #[test]
    fn empty_and_control_code_is_retained_only_in_static_reader_metadata() {
        let md = "```\n```\n\n```\n{\\rtf1}\n%!PS-Adobe-3.0\n\x1b]52;payload\x07\n```";
        let doc = Document::new(md, Theme::default());
        assert_eq!(doc.code_blocks()[0].text, "");
        assert!(doc.code_blocks()[1].text.contains("\x1b]52;payload\x07"));
        let lines = doc.layout(80, RenderOptions::default());
        assert!(!lines.iter().any(|line| line.plain_text().contains('\x1b')));
        for top in parse::parse(md, &Theme::default()) {
            if let Block::Code(code) = top.block {
                assert!(code.copy_text.is_none());
            }
        }
    }

    #[test]
    fn searches_semantics_across_styles_and_wraps_without_decorations() {
        let doc = Document::new(
            "# Title\n\n中**文** English abcdefghij [label](hidden-url)\n",
            Theme::default(),
        );
        assert_eq!(doc.find("中文").len(), 1);
        assert!(doc.find("hidden-url").is_empty());
        assert!(doc.find("**").is_empty());
        let found = doc.find("abcdefghij");
        for width in [4, 8, 80] {
            let lines = doc.layout(width, RenderOptions::default());
            assert!(Document::match_line(&lines, &found[0]).is_some());
        }
    }
    #[test]
    fn nested_matches_map_to_content_not_the_container_start() {
        let doc = Document::new(
            "- one\n\n  many words before the matching needle\n\n> quoted needle\n",
            Theme::default(),
        );
        let lines = doc.layout(15, RenderOptions::default());
        let matches = doc.find("needle");
        assert_eq!(matches.len(), 2);
        for m in matches {
            let row = Document::match_line(&lines, &m).unwrap();
            assert!(lines[row].plain_text().contains("needle"));
        }
    }
    #[test]
    fn indexed_layout_preserves_existing_rendering_and_source_contract() {
        let md = include_str!("../../../fixtures/acceptance.md");
        let theme = Theme::default();
        let doc = Document::new(md, theme.clone());
        for width in [1, 4, 20, 80] {
            for large in [false, true] {
                let options = RenderOptions {
                    large_headings: large,
                };
                let normal = crate::render_with_options(md, width, &theme, options);
                let indexed = doc.layout(width, options);
                assert_eq!(normal.len(), indexed.len());
                for (a, b) in normal.iter().zip(indexed) {
                    assert_eq!(
                        (a.plain_text(), a.src.clone(), a.size),
                        (b.plain_text(), b.src, b.size)
                    );
                    assert!(
                        a.spans
                            .iter()
                            .all(|span| span.text_ranges.is_empty() && span.code_block.is_none())
                    );
                }
            }
        }
    }

    #[test]
    fn repeated_emoji_and_styled_table_cells_have_stable_matches() {
        let doc = Document::new(
            "# 👩‍💻 heading\n\n| **中文** needle | one |\n|---|---|\n| needle | 👩‍💻 |\n",
            Theme::default(),
        );
        assert_eq!(doc.find("needle").len(), 2);
        assert_eq!(doc.find("👩‍💻").len(), 2);
        for width in [1, 4, 20, 80] {
            let lines = doc.layout(
                width,
                RenderOptions {
                    large_headings: true,
                },
            );
            for query in ["needle", "👩‍💻", "中文"] {
                for found in doc.find(query) {
                    assert!(
                        Document::match_line(&lines, &found).is_some(),
                        "{query} at {width}"
                    );
                }
            }
        }
    }

    #[test]
    fn elided_table_ranges_never_include_other_wrapped_rows() {
        let doc = Document::new(
            "| abcdefghij | target |\n|---|---|\n| one | two |",
            Theme::default(),
        );
        let lines = doc.layout(5, RenderOptions::default());
        let found = doc.find("ef");
        let row = Document::match_line(&lines, &found[0]).unwrap();
        assert_eq!(lines[row].plain_text(), "│ ef…");
    }

    #[test]
    fn code_and_clipped_table_matches_keep_their_rows() {
        let doc = Document::new(
            "```rust\nfn main() { needle(); }\n```\n\n| a | hidden needle |\n|---|---|\n| b | x |",
            Theme::default(),
        );
        let lines = doc.layout(5, RenderOptions::default());
        for m in doc.find("needle") {
            assert!(Document::match_line(&lines, &m).is_some());
        }
    }
}
