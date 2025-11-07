/// Simple markdown parser for LSP hover tooltips
/// Handles common markdown elements: code blocks, inline code, bold, italic, headers, links

pub fn parse_markdown(markdown: &str) -> String {
    let mut html = String::new();
    let mut in_code_block = false;
    let mut code_block_lang = String::new();
    let mut code_block_content = String::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Handle code blocks
        if line.starts_with("```") {
            if in_code_block {
                // End of code block
                html.push_str(&format!(
                    "<pre><code class=\"language-{}\">{}</code></pre>",
                    code_block_lang,
                    escape_html(&code_block_content)
                ));
                code_block_content.clear();
                code_block_lang.clear();
                in_code_block = false;
            } else {
                // Start of code block
                in_code_block = true;
                code_block_lang = line[3..].trim().to_string();
            }
            i += 1;
            continue;
        }

        if in_code_block {
            code_block_content.push_str(line);
            code_block_content.push('\n');
            i += 1;
            continue;
        }

        // Handle headers
        if line.starts_with("# ") {
            html.push_str(&format!("<h1>{}</h1>", parse_inline(&line[2..])));
        } else if line.starts_with("## ") {
            html.push_str(&format!("<h2>{}</h2>", parse_inline(&line[3..])));
        } else if line.starts_with("### ") {
            html.push_str(&format!("<h3>{}</h3>", parse_inline(&line[4..])));
        } else if line.starts_with("#### ") {
            html.push_str(&format!("<h4>{}</h4>", parse_inline(&line[5..])));
        } else if line.trim().is_empty() {
            html.push_str("<br>");
        } else {
            // Regular paragraph
            html.push_str(&format!("<p>{}</p>", parse_inline(line)));
        }

        i += 1;
    }

    // Close any open code block
    if in_code_block {
        html.push_str(&format!(
            "<pre><code class=\"language-{}\">{}</code></pre>",
            code_block_lang,
            escape_html(&code_block_content)
        ));
    }

    html
}

fn parse_inline(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.char_indices().peekable();
    let text_bytes = text.as_bytes();

    while let Some((i, ch)) = chars.next() {
        // Handle inline code `code`
        if ch == '`' {
            let start = i;
            let mut code_content = String::new();
            let mut found_closing = false;

            while let Some((j, c)) = chars.next() {
                if c == '`' {
                    result.push_str(&format!("<code>{}</code>", escape_html(&code_content)));
                    found_closing = true;
                    break;
                } else {
                    code_content.push(c);
                }
            }

            if !found_closing {
                // No closing backtick, treat as literal
                result.push('`');
                result.push_str(&code_content);
            }
            continue;
        }

        // Handle bold **text**
        if ch == '*' && chars.peek().map(|(_, c)| *c) == Some('*') {
            chars.next(); // Skip second *
            let start = i;
            let mut bold_content = String::new();
            let mut found_closing = false;

            while let Some((j, c)) = chars.next() {
                if c == '*' && chars.peek().map(|(_, c)| *c) == Some('*') {
                    chars.next(); // Skip second *
                    result.push_str(&format!("<strong>{}</strong>", parse_inline(&bold_content)));
                    found_closing = true;
                    break;
                } else {
                    bold_content.push(c);
                }
            }

            if !found_closing {
                // No closing **, treat as literal
                result.push_str("**");
                result.push_str(&bold_content);
            }
            continue;
        }

        // Handle italic *text* (but not **text** which is bold)
        if ch == '*' && chars.peek().map(|(_, c)| *c) != Some('*') {
            let start = i;
            let mut italic_content = String::new();
            let mut found_closing = false;

            while let Some((j, c)) = chars.next() {
                if c == '*' && chars.peek().map(|(_, c)| *c) != Some('*') {
                    result.push_str(&format!("<em>{}</em>", parse_inline(&italic_content)));
                    found_closing = true;
                    break;
                } else if c == '*' {
                    // Found **, this is bold, not italic
                    chars.next(); // Skip second *
                    italic_content.push('*');
                    italic_content.push('*');
                    continue;
                } else {
                    italic_content.push(c);
                }
            }

            if !found_closing {
                // No closing *, treat as literal
                result.push('*');
                result.push_str(&italic_content);
            }
            continue;
        }

        // Handle links [text](url)
        if ch == '[' {
            let mut link_text = String::new();
            let mut found_closing_bracket = false;

            while let Some((_, c)) = chars.next() {
                if c == ']' {
                    found_closing_bracket = true;
                    break;
                } else {
                    link_text.push(c);
                }
            }

            if found_closing_bracket && chars.peek().map(|(_, c)| *c) == Some('(') {
                chars.next(); // Skip (
                let mut link_url = String::new();
                let mut found_closing_paren = false;

                while let Some((_, c)) = chars.next() {
                    if c == ')' {
                        found_closing_paren = true;
                        break;
                    } else {
                        link_url.push(c);
                    }
                }

                if found_closing_paren {
                    result.push_str(&format!(
                        "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">{}</a>",
                        escape_html(&link_url),
                        parse_inline(&link_text)
                    ));
                    continue;
                }
            }

            // No valid link, treat as literal
            result.push('[');
            result.push_str(&link_text);
            if found_closing_bracket {
                result.push(']');
            }
            continue;
        }

        result.push(ch);
    }

    result
}

fn escape_html(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '&' => "&amp;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#x27;".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

