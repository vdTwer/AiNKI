//! Card text is Markdown. Raw HTML is not allowed: it is escaped before
//! rendering and the output is sanitized again.

use pulldown_cmark::{Event, Options, Parser, html};

pub fn render(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(src, opts).map(|e| match e {
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        // Single newlines become line breaks: what people expect on a card.
        Event::SoftBreak => Event::HardBreak,
        e => e,
    });
    let mut out = String::new();
    html::push_html(&mut out, parser);
    ammonia::Builder::default()
        .rm_tags(&["img"])
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(&out)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn renders_markdown_and_strips_html() {
        let html = render("**bold**\n<script>alert(1)</script>");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn newline_is_break() {
        assert!(render("a\nb").contains("<br>"));
    }
}
