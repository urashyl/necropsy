//! Markdown and code rendering for pack content.

use pulldown_cmark::{html, Event, Options, Parser};
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// CSS class prefix for highlighted tokens, see `static/app.css`.
const CLASS_PREFIX: &str = "s-";

/// Renders markdown to HTML. Raw HTML in the source is turned into text, so a
/// pack cannot inject markup into the page.
pub fn render_markdown(src: &str) -> String {
    let parser =
        Parser::new_ext(src, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH).map(|ev| {
            match ev {
                Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
                other => other,
            }
        });
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

pub struct Highlighter {
    syntaxes: SyntaxSet,
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            syntaxes: SyntaxSet::load_defaults_newlines(),
        }
    }

    /// Highlighted HTML for `text`, picking the language from the file
    /// extension. Unknown extensions render as plain text. The output is
    /// already escaped and safe to embed.
    pub fn highlight(&self, file_name: &str, text: &str) -> String {
        let ext = file_name.rsplit('.').next().unwrap_or("");
        let syntax = self
            .syntaxes
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        let mut gen = ClassedHTMLGenerator::new_with_class_style(
            syntax,
            &self.syntaxes,
            ClassStyle::SpacedPrefixed {
                prefix: CLASS_PREFIX,
            },
        );
        for line in LinesWithEndings::from(text) {
            if gen
                .parse_html_for_line_which_includes_newline(line)
                .is_err()
            {
                return escape(text);
            }
        }
        gen.finalize()
    }
}

/// Number of lines shown in the gutter for `text`.
pub fn line_count(text: &str) -> usize {
    text.lines().count().max(1)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_html_is_not_passed_through() {
        let out = render_markdown("hi <script>alert(1)</script>\n\n<div onclick=x>y</div>");
        assert!(!out.contains("<script"));
        assert!(!out.contains("<div"));
        assert!(out.contains("&lt;script&gt;"));
    }

    #[test]
    fn highlighting_escapes_source() {
        let h = Highlighter::new();
        let out = h.highlight("x.js", "const a = \"<img src=x onerror=1>\";\n");
        assert!(!out.contains("<img"));
        assert!(out.contains("&lt;img"));
    }

    #[test]
    fn unknown_extension_is_plain_text() {
        let h = Highlighter::new();
        assert!(h.highlight("data.zzz", "a < b\n").contains("a &lt; b"));
    }
}
