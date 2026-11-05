//! Markdown rendering core for tmd.
//!
//! Turns markdown into styled lines. This crate must not depend on any
//! terminal library; output backends live in `tmd-cli`.

mod code;
mod document;
mod highlight;
mod html;
mod layout;
mod line;
mod parse;
mod streamer;
mod style;
mod table;
mod theme;
mod theme_file;
mod width;
mod wrap;

pub use document::{CodeSnippet, Document, Heading};
pub use line::{Line, LineSize, Span};
pub use streamer::{Frame, Streamer};
pub use style::{Color, Style};
pub use theme::{Symbols, TableSymbols, Theme};
pub use theme_file::ThemeError;
pub use width::{clusters, text_width, truncate};

/// Renders `md` into rows at most `width` columns wide.
///
/// Top-level blocks are separated by one blank row, which belongs to the
/// block below it: it shares the source range of that block's first row.
/// Whether the row exists depends on the block below rendering anything
/// (an HTML block of bare tags renders nothing), so streaming commits it
/// together with that block.
pub fn render(md: &str, width: u16, theme: &Theme) -> Vec<Line> {
    render_with_options(md, width, theme, RenderOptions::default())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions {
    /// Double the size of top-level H1 text. Underlines remain normal.
    /// Callers must check output support before enabling this.
    pub large_headings: bool,
}

/// Like [`render`], with explicit layout options. Widths below four columns
/// fall back to normal layout, allowing one wide character at double size.
pub fn render_with_options(
    md: &str,
    width: u16,
    theme: &Theme,
    options: RenderOptions,
) -> Vec<Line> {
    render_blocks(&parse::parse(md, theme), width, theme, options)
}

fn render_blocks(
    blocks: &[parse::TopBlock],
    width: u16,
    theme: &Theme,
    options: RenderOptions,
) -> Vec<Line> {
    let width = usize::from(width.max(1));
    let ctx = layout::Ctx {
        theme,
        list_depth: 0,
    };
    let mut lines = Vec::new();
    for top_block in blocks {
        let rows: Vec<Line> = if options.large_headings
            && width >= 4
            && let parse::Block::Heading { level: 1, spans } = &top_block.block
        {
            let mut rows: Vec<Line> = wrap::wrap(spans, width / 2)
                .into_iter()
                .map(|spans| Line {
                    spans,
                    src: top_block.src.clone(),
                    size: LineSize::Double,
                })
                .collect();
            if !theme.symbols.h1_rule.is_empty() {
                rows.push(Line {
                    spans: layout::rule_row(&theme.symbols.h1_rule, &theme.h1, width),
                    src: top_block.src.clone(),
                    size: LineSize::Normal,
                });
            }
            rows
        } else if let parse::Block::Code(code) = &top_block.block {
            // Code lines keep their own source ranges so streaming can
            // commit a long block line by line.
            code::rows(theme, code, width)
                .into_iter()
                .map(|(spans, src)| Line {
                    spans,
                    src,
                    size: LineSize::Normal,
                })
                .collect()
        } else {
            layout::block(&ctx, &top_block.block, width)
                .into_iter()
                .map(|spans| Line {
                    spans,
                    src: top_block.src.clone(),
                    size: LineSize::Normal,
                })
                .collect()
        };
        if let (false, Some(first)) = (lines.is_empty(), rows.first()) {
            lines.push(Line {
                spans: Vec::new(),
                src: first.src.clone(),
                size: LineSize::Normal,
            });
        }
        lines.extend(rows);
    }
    lines
}

#[cfg(test)]
mod tests;
