use crate::screen::line::{is_digit, is_letter, is_whitespace};
use unicode_general_category::{get_general_category, GeneralCategory};

pub fn opens(text: &str) -> bool {
    let text: Vec<_> = text.encode_utf16().collect();
    matches(&text).iter().any(Option::is_some)
}

pub fn is_bare_label(text: &str) -> bool {
    let mut text: Vec<_> = text.encode_utf16().collect();
    while text.last().is_some_and(|&c| is_whitespace(c)) {
        text.pop();
    }
    matches(&text).contains(&Some(text.len()))
}

fn matches(text: &[u16]) -> [Option<usize>; 4] {
    [timestamp(text), channel(text), handle(text), name(text)]
}
fn skip_white(text: &[u16], mut i: usize) -> usize {
    while text.get(i).is_some_and(|&c| is_whitespace(c)) {
        i += 1;
    }
    i
}
fn colon(c: u16) -> bool {
    matches!(c, 0x003a | 0xff1a)
}

fn timestamp(text: &[u16]) -> Option<usize> {
    let mut i = skip_white(text, 0);
    if !text
        .get(i)
        .is_some_and(|c| matches!(c, 0x005b | 0x3010 | 0x0028))
    {
        return None;
    }
    i = skip_white(text, i + 1);
    let start = i;
    while i - start < 2 && text.get(i).is_some_and(|&c| is_digit(c)) {
        i += 1;
    }
    if i == start {
        return None;
    }
    i = skip_white(text, i);
    if !text.get(i).is_some_and(|&c| colon(c)) {
        return None;
    }
    i = skip_white(text, i + 1);
    if !text.get(i).is_some_and(|&c| is_digit(c)) || !text.get(i + 1).is_some_and(|&c| is_digit(c))
    {
        return None;
    }
    i = skip_white(text, i + 2);
    text.get(i)
        .is_some_and(|c| matches!(c, 0x005d | 0x3011 | 0x0029))
        .then_some(i + 1)
}

fn channel(text: &[u16]) -> Option<usize> {
    let start = skip_white(text, 0);
    if text
        .get(start)
        .is_some_and(|c| matches!(c, 0x005b | 0x3010))
    {
        let first = start + 1;
        for count in 1..=18 {
            let Some(&unit) = text.get(first + count - 1) else {
                break;
            };
            if is_whitespace(unit) || colon(unit) {
                break;
            }
            if count < 2 {
                continue;
            }
            let close = first + count;
            if text
                .get(close)
                .is_some_and(|c| matches!(c, 0x005d | 0x3011 | 0x0029 | 0x0031 | 0x007c))
            {
                if let Some(end) = channel_label(text, close + 1) {
                    return Some(end);
                }
            }
        }
    }
    if text
        .get(start)
        .is_some_and(|c| matches!(c, 0x0049 | 0x004c | 0x0054 | 0x0069 | 0x006c | 0x0074))
    {
        let first = start + 1;
        let mut count = 0;
        while count < 12
            && text
                .get(first + count)
                .is_some_and(|c| matches!(c,0x0041..=0x005a|0x0061..=0x007a|0x212a))
        {
            count += 1;
        }
        if count >= 3 {
            let close = first + count;
            if text
                .get(close)
                .is_some_and(|c| matches!(c, 0x0031 | 0x0029 | 0x005d))
            {
                return channel_label(text, close + 1);
            }
        }
    }
    None
}

fn channel_label(text: &[u16], prefix_end: usize) -> Option<usize> {
    let mut start = skip_white(text, prefix_end);
    let mut end = start;
    while text
        .get(end)
        .is_some_and(|&c| !colon(c) && c != u16::from(b';'))
    {
        end += 1;
    }
    if end == start && start > prefix_end {
        start -= 1;
    }
    (end > start && end - start <= 40 && text.get(end).is_some()).then_some(end + 1)
}

fn handle(text: &[u16]) -> Option<usize> {
    let mut i = skip_white(text, 0);
    if text.get(i) != Some(&u16::from(b'@')) {
        return None;
    }
    i += 1;
    let start = i;
    while i - start < 40 && text.get(i).is_some_and(|&c| !is_whitespace(c) && !colon(c)) {
        i += 1;
    }
    if i == start {
        return None;
    }
    i = skip_white(text, i);
    text.get(i).is_some_and(|&c| colon(c)).then_some(i + 1)
}

fn name(text: &[u16]) -> Option<usize> {
    let mut i = skip_white(text, 0);
    let first = *text.get(i)?;
    let number = char::from_u32(u32::from(first)).is_some_and(|c| {
        matches!(
            get_general_category(c),
            GeneralCategory::DecimalNumber
                | GeneralCategory::LetterNumber
                | GeneralCategory::OtherNumber
        )
    });
    if !is_letter(first) && !number && first != u16::from(b'_') {
        return None;
    }
    i += 1;
    let start = i;
    while i - start < 31
        && text.get(i).is_some_and(|&c| {
            !is_whitespace(c)
                && !matches!(
                    c,
                    0x003a | 0xff1a | 0x002c | 0xff0c | 0x3002 | 0x0021 | 0x003f | 0xff01 | 0xff1f
                )
        })
    {
        i += 1;
    }
    i = skip_white(text, i);
    if !text.get(i).is_some_and(|&c| colon(c)) {
        return None;
    }
    let end = i + 1;
    if text.get(end) == Some(&u16::from(b'/')) && text.get(end + 1) == Some(&u16::from(b'/')) {
        return None;
    }
    for j in end..text.len() {
        if text[j] == u16::from(b'\n') {
            break;
        }
        if text[j] == u16::from(b';') && text[j + 1..].iter().all(|&c| is_whitespace(c)) {
            return None;
        }
    }
    Some(end)
}
