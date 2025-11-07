/// Highlight SimplicityHL code with HTML spans
pub fn highlight_code(code: &str) -> String {
    if code.is_empty() {
        return String::new();
    }
    
    // Keywords that should be highlighted
    let keywords = [
        "let", "mut", "if", "else", "match", "fn", "return", "pub", "struct", "enum",
        "impl", "use", "as", "for", "while", "loop", "break", "continue", "const",
        "static", "trait", "where", "type", "mod", "crate", "self", "super", "in",
        "true", "false", "Some", "None", "Ok", "Err", "Option", "Result",
    ];

    let mut result = String::with_capacity(code.len() * 2);
    let mut chars = code.chars().peekable();
    let mut in_string = false;
    let mut in_char = false;
    let mut in_comment = false;
    let mut in_line_comment = false;
    let mut string_delimiter = '"';
    let mut buffer = String::new();
    let mut expect_fn_name = false; // after seeing `fn`, next identifier is a function name

    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';

    while let Some(ch) = chars.next() {
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
                result.push_str(&format!("<span class=\"hl-comment\">{}</span>", escape_html(&buffer)));
                buffer.clear();
                result.push(escape_char(ch));
                continue;
            }
            buffer.push(ch);
            continue;
        }

        if in_comment {
            buffer.push(ch);
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next(); // consume '/'
                in_comment = false;
                result.push_str(&format!("<span class=\"hl-comment\">{}</span>", escape_html(&buffer)));
                buffer.clear();
            }
            continue;
        }

        if in_string || in_char {
            buffer.push(ch);
            if in_string && ch == string_delimiter || in_char && ch == '\'' {
                // Check if it's escaped
                let mut backslash_count = 0;
                let mut check_pos = buffer.len().saturating_sub(2);
                while check_pos < buffer.len() && check_pos > 0 {
                    if let Some(c) = buffer.chars().nth(check_pos) {
                        if c == '\\' {
                            backslash_count += 1;
                            check_pos = check_pos.saturating_sub(1);
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                
                if backslash_count % 2 == 0 {
                    // Not escaped, end of string/char
                    result.push_str(&format!("<span class=\"hl-string\">{}</span>", escape_html(&buffer)));
                    buffer.clear();
                    in_string = false;
                    in_char = false;
                }
            }
            continue;
        }

        // Check for string/char start
        if ch == '"' || ch == '\'' {
            if buffer.ends_with('r') || buffer.ends_with("r#") {
                // Raw string literal
                buffer.push(ch);
                continue;
            }
            in_string = ch == '"';
            in_char = ch == '\'';
            string_delimiter = ch;
            buffer.push(ch);
            continue;
        }

        // Check for comments
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next(); // consume second '/'
            buffer.push_str("//");
            in_line_comment = true;
            continue;
        }

        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next(); // consume '*'
            buffer.push_str("/*");
            in_comment = true;
            continue;
        }

        // Check if we're building a word
        if is_ident_char(ch) {
            buffer.push(ch);
        } else {
            // If we have an identifier in buffer and encounter '::', treat the whole path as namespace
            if !buffer.is_empty() && ch == ':' && chars.peek() == Some(&':') {
                let mut path = String::new();
                path.push_str(&buffer);
                buffer.clear();
                // consume the second ':'
                let _ = chars.next();
                path.push_str("::");
                // consume following segments `ident(::ident)*`
                loop {
                    // consume identifier chars
                    while let Some(&c2) = chars.peek() {
                        if is_ident_char(c2) {
                            path.push(c2);
                            let _ = chars.next();
                        } else {
                            break;
                        }
                    }
                    // check for another '::'
                    let mut it = chars.clone();
                    if it.next() == Some(':') && it.next() == Some(':') {
                        // consume both ':' on original iterator
                        let _ = chars.next();
                        let _ = chars.next();
                        path.push_str("::");
                        continue;
                    }
                    break;
                }
                // Only color namespaces that start with `jet::`; keep others (e.g., param::) white
                if path.starts_with("jet::") {
                    result.push_str(&format!("<span class=\"hl-namespace\">{}</span>", escape_html(&path)));
                } else {
                    result.push_str(&escape_html(&path));
                }
                continue; // skip default handling of this ':'
            }

            // Process accumulated word
            if !buffer.is_empty() {
                let word = buffer.as_str();
                
                // Check if it's a keyword
                if keywords.contains(&word) {
                    let highlighted = format!("<span class=\"hl-keyword\">{}</span>", escape_html(word));
                    result.push_str(&highlighted);
                    if word == "fn" { expect_fn_name = true; }
                } else if expect_fn_name {
                    // Color the identifier after `fn` as a function name
                    result.push_str(&format!("<span class=\"hl-function\">{}</span>", escape_html(word)));
                    expect_fn_name = false;
                } else if word.starts_with("0x") || word.starts_with("0b") || word.starts_with("0o") {
                    // Number literal
                    result.push_str(&format!("<span class=\"hl-number\">{}</span>", escape_html(word)));
                } else if word.parse::<f64>().is_ok() || word.parse::<i64>().is_ok() {
                    // Number literal
                    result.push_str(&format!("<span class=\"hl-number\">{}</span>", escape_html(word)));
                } else {
                    // Regular identifier
                    result.push_str(&escape_html(word));
                }
                buffer.clear();
            }

            // Special handling: If we just placed a keyword other than `fn`, we should not carry expect_fn_name
            if ch == '(' || ch == '{' || ch == ';' || ch == '\n' { expect_fn_name = false; }

            // Handle operators and punctuation
            match ch {
                '+' | '-' | '*' | '/' | '%' | '=' | '!' | '<' | '>' | '&' | '|' | '^' | '~' | '?' | ':' => {
                    result.push_str(&format!("<span class=\"hl-operator\">{}</span>", escape_char(ch)));
                }
                '(' | ')' | '[' | ']' | '{' | '}' => {
                    // Brackets and parentheses - render directly without escaping
                    result.push(ch);
                }
                '.' | ',' | ';' | '@' | '#' => {
                    result.push(ch);
                }
                '\n' | '\r' | '\t' | ' ' => {
                    result.push(ch);
                }
                _ => {
                    result.push(escape_char(ch));
                }
            }
        }
    }

    // Handle remaining buffer
    if !buffer.is_empty() {
        if in_string || in_char {
            result.push_str(&format!("<span class=\"hl-string\">{}</span>", escape_html(&buffer)));
        } else if in_comment || in_line_comment {
            result.push_str(&format!("<span class=\"hl-comment\">{}</span>", escape_html(&buffer)));
        } else {
            let word = buffer.as_str();
            if keywords.contains(&word) {
                result.push_str(&format!("<span class=\"hl-keyword\">{}</span>", escape_html(word)));
            } else if expect_fn_name {
                result.push_str(&format!("<span class=\"hl-function\">{}</span>", escape_html(word)));
            } else if word.parse::<f64>().is_ok() || word.parse::<i64>().is_ok() {
                result.push_str(&format!("<span class=\"hl-number\">{}</span>", escape_html(word)));
            } else {
                result.push_str(&escape_html(word));
            }
        }
    }

    result
}

// Escape HTML special characters
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

// Escape a single character
fn escape_char(ch: char) -> char {
    // For most characters, we can just return them as-is
    // Only escape in HTML context when needed
    ch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyword_highlighting() {
        let code = "let x = 5;";
        let result = highlight_code(code);
        assert!(result.contains("hl-keyword"));
        assert!(result.contains("let"));
    }

    #[test]
    fn test_string_highlighting() {
        let code = r#"let s = "hello";"#;
        let result = highlight_code(code);
        assert!(result.contains("hl-string"));
    }

    #[test]
    fn test_comment_highlighting() {
        let code = "// This is a comment";
        let result = highlight_code(code);
        assert!(result.contains("hl-comment"));
    }
}
