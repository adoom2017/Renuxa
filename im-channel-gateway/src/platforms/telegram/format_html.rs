//! Markdown → Telegram HTML (ported from Python `format_html.py`).

use std::cell::RefCell;

use regex::Regex;

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Convert standard Markdown to Telegram Bot API HTML subset.
pub fn markdown_to_telegram_html(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }

    let placeholders = RefCell::new(Vec::<String>::new());
    let mut text = text.to_string();

    let push_ph = |ph: &RefCell<Vec<String>>, html: String| -> String {
        let mut guard = ph.borrow_mut();
        let idx = guard.len();
        guard.push(html);
        format!("\x00PH{idx}\x00")
    };

    // Fenced code blocks
    let re_code = Regex::new(r"```(\w*)\n?(.*?)```").expect("code block regex");
    text = re_code
        .replace_all(&text, |caps: &regex::Captures| {
            let lang = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let code = escape_html(caps.get(2).map(|m| m.as_str()).unwrap_or(""));
            if !lang.is_empty() {
                push_ph(
                    &placeholders,
                    format!(
                        "<pre><code class=\"language-{}\">{code}</code></pre>",
                        escape_html(lang)
                    ),
                )
            } else {
                push_ph(&placeholders, format!("<pre>{code}</pre>"))
            }
        })
        .into_owned();

    // Inline code
    let re_inline = Regex::new(r"`([^`\n]+)`").expect("inline code regex");
    text = re_inline
        .replace_all(&text, |caps: &regex::Captures| {
            let code = escape_html(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
            push_ph(&placeholders, format!("<code>{code}</code>"))
        })
        .into_owned();

    // Links
    let re_link = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").expect("link regex");
    text = re_link
        .replace_all(&text, |caps: &regex::Captures| {
            let link_text = escape_html(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
            let mut url = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            url = url.replace('<', "%3C").replace('>', "%3E");
            push_ph(&placeholders, format!("<a href=\"{url}\">{link_text}</a>"))
        })
        .into_owned();
    let placeholders = placeholders.into_inner();

    text = escape_html(&text);

    let re_hr = Regex::new(r"(?m)^[\*\-_]{3,}\s*$").expect("hr regex");
    text = re_hr.replace_all(&text, "———").into_owned();

    let re_header = Regex::new(r"(?m)^#{1,6}\s+(.+?)$").expect("header regex");
    text = re_header.replace_all(&text, "<b>$1</b>").into_owned();

    // Blockquotes (&gt; after escape)
    let lines: Vec<&str> = text.split('\n').collect();
    let mut result_lines: Vec<String> = Vec::new();
    let mut quote_buf: Vec<String> = Vec::new();
    let flush = |result: &mut Vec<String>, quote: &mut Vec<String>| {
        if !quote.is_empty() {
            result.push(format!("<blockquote>{}</blockquote>", quote.join("\n")));
            quote.clear();
        }
    };
    for line in lines {
        let stripped = line.trim_start();
        if let Some(rest) = stripped.strip_prefix("&gt; ") {
            quote_buf.push(rest.to_string());
        } else if stripped == "&gt;" {
            quote_buf.push(String::new());
        } else {
            flush(&mut result_lines, &mut quote_buf);
            result_lines.push(line.to_string());
        }
    }
    flush(&mut result_lines, &mut quote_buf);
    text = result_lines.join("\n");

    let re_ul = Regex::new(r"(?m)^(\s*)[\*\-]\s+").expect("ul regex");
    text = re_ul.replace_all(&text, "$1• ").into_owned();

    let re_spoiler = Regex::new(r"\|\|(.+?)\|\|").expect("spoiler regex");
    text = re_spoiler
        .replace_all(&text, "<tg-spoiler>$1</tg-spoiler>")
        .into_owned();

    let re_bold_italic = Regex::new(r"\*{3}(.+?)\*{3}").expect("bold italic regex");
    text = re_bold_italic
        .replace_all(&text, "<b><i>$1</i></b>")
        .into_owned();

    let re_bold = Regex::new(r"\*{2}(.+?)\*{2}").expect("bold regex");
    text = re_bold.replace_all(&text, "<b>$1</b>").into_owned();

    let re_bold_u = Regex::new(r"__(.+?)__").expect("bold underline regex");
    text = re_bold_u.replace_all(&text, "<b>$1</b>").into_owned();

    let re_italic = Regex::new(r"\*([^*\n]+)\*").expect("italic regex");
    text = re_italic.replace_all(&text, "<i>$1</i>").into_owned();

    let re_italic_u = Regex::new(r"_([^_\n]+)_").expect("italic u regex");
    text = re_italic_u.replace_all(&text, "<i>$1</i>").into_owned();

    let re_strike = Regex::new(r"~~(.+?)~~").expect("strike regex");
    text = re_strike.replace_all(&text, "<s>$1</s>").into_owned();

    for (idx, content) in placeholders.iter().enumerate() {
        text = text.replace(&format!("\x00PH{idx}\x00"), content);
    }

    text
}

/// Strip Markdown for plain-text fallback when HTML send fails.
pub fn strip_markdown(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let mut text = text.to_string();
    let re = |pat: &str| Regex::new(pat).unwrap();
    text = re(r"```\w*\n?").replace_all(&text, "").into_owned();
    text = re(r"`([^`]+)`").replace_all(&text, "$1").into_owned();
    text = re(r"(?m)^#{1,6}\s+").replace_all(&text, "").into_owned();
    text = re(r"(?m)^[\*\-_]{3,}\s*$")
        .replace_all(&text, "———")
        .into_owned();
    text = re(r"\*{1,3}(.+?)\*{1,3}")
        .replace_all(&text, "$1")
        .into_owned();
    text = re(r"_{1,2}(.+?)_{1,2}")
        .replace_all(&text, "$1")
        .into_owned();
    text = re(r"~~(.+?)~~").replace_all(&text, "$1").into_owned();
    text = re(r"\|\|(.+?)\|\|").replace_all(&text, "$1").into_owned();
    text = re(r"\[([^\]]+)\]\(([^)]+)\)")
        .replace_all(&text, "$1 ($2)")
        .into_owned();
    text = re(r"(?m)^>\s?").replace_all(&text, "").into_owned();
    text = re(r"(?m)^(\s*)[\*\-]\s+")
        .replace_all(&text, "$1• ")
        .into_owned();
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_and_code() {
        let html = markdown_to_telegram_html("**hi** and `x`");
        assert!(html.contains("<b>hi</b>"));
        assert!(html.contains("<code>x</code>"));
    }
}
