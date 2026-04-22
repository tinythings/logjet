pub(crate) fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn char_to_byte_idx(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map(|(idx, _)| idx).unwrap_or(text.len())
}

pub(super) fn insert_char_at(text: &mut String, cursor: &mut usize, ch: char) {
    let idx = char_to_byte_idx(text, *cursor);
    text.insert(idx, ch);
    *cursor += 1;
}

pub(super) fn delete_char_before(text: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let end = char_to_byte_idx(text, *cursor);
    let start = char_to_byte_idx(text, cursor.saturating_sub(1));
    text.replace_range(start..end, "");
    *cursor = cursor.saturating_sub(1);
}

pub(super) fn delete_char_at(text: &mut String, cursor: usize) {
    if cursor >= char_count(text) {
        return;
    }
    let start = char_to_byte_idx(text, cursor);
    let end = char_to_byte_idx(text, cursor + 1);
    text.replace_range(start..end, "");
}

pub(crate) fn text_preview(bytes: &[u8], limit: usize) -> String {
    trim_single_line(&String::from_utf8_lossy(bytes), limit)
}

pub(super) fn trim_single_line(input: &str, limit: usize) -> String {
    let flattened = input
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' => ' ',
            other if other.is_control() => ' ',
            other => other,
        })
        .collect::<String>();

    let mut output = flattened.chars().take(limit).collect::<String>();
    if flattened.chars().count() > limit {
        output.push_str("...");
    }
    output
}

pub(super) fn smart_wrap(text: &str, width: usize) -> String {
    if width == 0 {
        return text.to_string();
    }
    let text = text.replace('\t', "    ");
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    for (li, line) in text.split('\n').enumerate() {
        if li > 0 {
            out.push('\n');
        }
        let mut col = 0usize;
        for word in line.split(' ') {
            let wlen = word.chars().count();
            if wlen == 0 {
                if col > 0 && col < width {
                    out.push(' ');
                    col += 1;
                }
                continue;
            }
            if wlen > width {
                if col > 0 {
                    out.push('\n');
                }
                out.extend(word.chars().take(width.saturating_sub(1)));
                out.push('…');
                col = width;
            } else if col + usize::from(col > 0) + wlen > width {
                out.push('\n');
                out.push_str(word);
                col = wlen;
            } else {
                if col > 0 {
                    out.push(' ');
                    col += 1;
                }
                out.push_str(word);
                col += wlen;
            }
        }
    }
    out
}

pub(super) fn fit_to_width(input: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let char_count = input.chars().count();
    if char_count <= width {
        let mut padded = input.to_string();
        padded.push_str(&" ".repeat(width - char_count));
        return padded;
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut out = input.chars().take(width - 3).collect::<String>();
    out.push_str("...");
    out
}

pub(super) fn hex_preview(bytes: &[u8], limit: usize) -> String {
    let mut out = bytes.iter().take(limit).map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" ");
    if bytes.len() > limit {
        out.push_str(" ...");
    }
    out
}

pub(super) fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (chunk_index, chunk) in bytes.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}: ", chunk_index * 16));
        for byte in chunk {
            out.push_str(&format!("{byte:02x} "));
        }
        out.push('\n');
    }
    out
}
