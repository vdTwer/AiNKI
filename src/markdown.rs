//! Card text is Markdown. Raw HTML is not allowed: it is escaped before
//! rendering and the output is sanitized again.

use pulldown_cmark::{Event, Options, Parser, html};

/// The Markdown dialect of cards: CommonMark plus tables, strikethrough and
/// task lists. Splitting a multicard parses with the same options.
pub fn options() -> Options {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts
}

pub fn render(src: &str) -> String {
    let src = tidy(src);
    let parser = Parser::new_ext(&src, options()).map(|e| match e {
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

/// Lines that look like list items but are indented four spaces or more
/// right under a line of text are, in Markdown, more of that text: the
/// dashes would show. Text pasted from elsewhere often looks like that, so
/// such a run of lines loses its indent and becomes the list it was meant
/// to be. Nothing else is changed.
pub fn tidy(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut fence: Option<&str> = None;
    let mut n = 0;
    while n < lines.len() {
        let line = lines[n];
        let t = line.trim_start();
        if let Some(f) = fence {
            if t.starts_with(f) {
                fence = None;
            }
        } else if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(&t[..3]);
        } else if indent(line) < 4 && !t.is_empty() && !is_item(t) {
            // A run of indented items (and their own deeper lines) under it.
            let run = lines[n + 1..]
                .iter()
                .take_while(|l| indent(l) >= 4 && !l.trim().is_empty())
                .count();
            let block = &lines[n + 1..n + 1 + run];
            if block.first().is_some_and(|l| is_item(l.trim_start())) {
                let cut = block.iter().map(|l| indent(l)).min().unwrap_or(0);
                out.push(line.to_string());
                out.extend(block.iter().map(|l| dedent(l, cut)));
                n += 1 + run;
                continue;
            }
        }
        out.push(line.to_string());
        n += 1;
    }
    let mut s = out.join("\n");
    if src.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Leading spaces, with a tab as four.
fn indent(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

fn dedent(line: &str, cut: usize) -> String {
    let mut width = 0;
    let mut bytes = 0;
    for c in line.chars() {
        if width >= cut || !(c == ' ' || c == '\t') {
            break;
        }
        width += if c == '\t' { 4 } else { 1 };
        bytes += c.len_utf8();
    }
    format!("{}{}", " ".repeat(width.saturating_sub(cut)), &line[bytes..])
}

/// Starts like a list item: "-", "*" or "+", or a number with "." or ")",
/// then a space.
fn is_item(t: &str) -> bool {
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    let rest = if digits > 0 {
        match t[digits..].strip_prefix(['.', ')']) {
            Some(r) if digits <= 9 => r,
            _ => return false,
        }
    } else {
        match t.strip_prefix(['-', '*', '+']) {
            Some(r) => r,
            None => return false,
        }
    };
    rest.starts_with([' ', '\t'])
}

#[cfg(test)]
mod tests {
    use super::{render, tidy};

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

    #[test]
    fn indented_items_under_text_become_a_list() {
        let src = "will\n    - now: I'll help.\n      more\n    - later\n\nkeep\n\n    code block";
        assert_eq!(tidy(src), "will\n- now: I'll help.\n  more\n- later\n\nkeep\n\n    code block");
        assert!(render(src).contains("<li>now: I'll help.<br>"));
        // A nested list stays nested.
        let nested = "- a\n    - b";
        assert_eq!(tidy(nested), nested);
        let fenced = "```\nx\n    - y\n```";
        assert_eq!(tidy(fenced), fenced);
    }
}
