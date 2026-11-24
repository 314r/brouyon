use crate::store::is_fence;
use std::ops::Range;

#[derive(Debug, PartialEq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn moved(&self, offset: usize) -> usize {
        if offset >= self.range.end {
            offset - self.range.len() + self.text.len()
        } else if offset <= self.range.start {
            offset
        } else {
            self.range.start + self.text.len()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Marker {
    Bullet(char),
    Number(u64, char),
}

impl Marker {
    fn next(self) -> String {
        match self {
            Marker::Bullet(bullet) => format!("{bullet} "),
            Marker::Number(number, delimiter) => {
                format!("{}{delimiter} ", number.saturating_add(1))
            }
        }
    }
}

#[derive(Debug, PartialEq)]
struct Item<'a> {
    indent: &'a str,
    marker: Marker,
    after_marker: usize,
    task: Option<(usize, bool)>,
    content: &'a str,
}

impl Item<'_> {
    fn next_prefix(&self) -> String {
        let task = if self.task.is_some() { "[ ] " } else { "" };
        format!("{}{}{task}", self.indent, self.marker.next())
    }
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

fn is_rule(line: &str) -> bool {
    let mut marks = line.chars().filter(|c| !c.is_whitespace());
    let Some(first) = marks.next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_') && marks.clone().count() >= 2 && marks.all(|c| c == first)
}

fn item(line: &str) -> Option<Item<'_>> {
    let indent = indent_of(line);
    let rest = &line[indent.len()..];
    if is_rule(rest) {
        return None;
    }
    let (marker, marker_len) = match rest.chars().next()? {
        bullet @ ('-' | '*' | '+') => (Marker::Bullet(bullet), 1),
        '0'..='9' => {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let delimiter = rest[digits..].chars().next()?;
            // Markdown stops counting at nine digits, which also keeps this in range.
            if digits > 9 || !matches!(delimiter, '.' | ')') {
                return None;
            }
            (
                Marker::Number(rest[..digits].parse().ok()?, delimiter),
                digits + 1,
            )
        }
        _ => return None,
    };
    let after = rest[marker_len..].strip_prefix(' ')?;
    let after_marker = line.len() - after.len();
    let ticked = match after.get(..3) {
        Some("[ ]") => Some(false),
        Some("[x]" | "[X]") => Some(true),
        _ => None,
    };
    let (task, content) = match ticked {
        Some(ticked) if after.len() == 3 || after[3..].starts_with(' ') => (
            Some((after_marker + 1, ticked)),
            after[3..].trim_start_matches(' '),
        ),
        _ => (None, after),
    };
    Some(Item {
        indent,
        marker,
        after_marker,
        task,
        content,
    })
}

fn in_code(before: &str) -> bool {
    before.lines().filter(|line| is_fence(line)).count() % 2 == 1
}

fn line_at(text: &str, offset: usize) -> (usize, &str) {
    let start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |newline| offset + newline);
    (start, text[start..end].trim_end_matches('\r'))
}

fn parent<'a>(text: &'a str, line_start: usize, indent: &str) -> Option<Item<'a>> {
    text[..line_start]
        .lines()
        .rev()
        .take_while(|line| !line.trim().is_empty())
        .filter_map(item)
        .find(|above| above.indent.len() < indent.len())
}

pub fn after_enter(text: &str, cursor: usize) -> Option<Edit> {
    let (new_start, _) = line_at(text, cursor);
    if new_start == 0 || !text[new_start..cursor].trim().is_empty() {
        return None;
    }
    let (above_start, above) = line_at(text, new_start - 1);
    if in_code(&text[..above_start]) {
        return None;
    }
    let item = item(above)?;
    let rest = line_at(text, cursor).1;
    let rest = &rest[cursor - new_start..];
    if item.content.trim().is_empty() && rest.trim().is_empty() {
        let outer = parent(text, above_start, item.indent);
        return Some(Edit {
            range: above_start..cursor,
            text: outer.map_or(String::new(), |outer| outer.next_prefix()),
        });
    }
    Some(Edit {
        range: new_start..cursor,
        text: item.next_prefix(),
    })
}

pub fn toggle_task(text: &str, cursor: usize) -> Option<Edit> {
    let (start, line) = line_at(text, cursor);
    if in_code(&text[..start]) || is_fence(line) {
        return None;
    }
    let insert = |at: usize, text: &str| Edit {
        range: start + at..start + at,
        text: text.into(),
    };
    Some(match item(line) {
        Some(Item {
            task: Some((mark, ticked)),
            ..
        }) => Edit {
            range: start + mark..start + mark + 1,
            text: if ticked { " " } else { "x" }.into(),
        },
        Some(item) => insert(item.after_marker, "[ ] "),
        None => insert(indent_of(line).len(), "- [ ] "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enter(text: &str) -> String {
        let at = text.find('|').unwrap();
        let (line_start, line) = line_at(text, at);
        let carried = indent_of(line).len().min(at - line_start);
        let mut after = format!("{}\n{}{}", &text[..at], &line[..carried], &text[at + 1..]);
        let mut cursor = at + 1 + carried;
        if let Some(edit) = after_enter(&after, cursor) {
            cursor = edit.moved(cursor);
            after.replace_range(edit.range, &edit.text);
        }
        after.insert(cursor, '|');
        after
    }

    fn toggle(text: &str) -> String {
        let cursor = text.find('|').unwrap();
        let mut after = text.replacen('|', "", 1);
        let edit = toggle_task(&after, cursor).expect("an edit");
        let cursor = edit.moved(cursor);
        after.replace_range(edit.range, &edit.text);
        after.insert(cursor, '|');
        after
    }

    #[test]
    fn enter_after_an_item_starts_the_next_one() {
        for (before, after) in [
            ("- milk|", "- milk\n- |"),
            ("* milk|", "* milk\n* |"),
            ("+ milk|", "+ milk\n+ |"),
            ("1. first|", "1. first\n2. |"),
            ("9) ninth|", "9) ninth\n10) |"),
            ("- [ ] call|", "- [ ] call\n- [ ] |"),
            ("- [x] done|", "- [x] done\n- [ ] |"),
            ("3. [X] done|", "3. [X] done\n4. [ ] |"),
            ("  - nested|", "  - nested\n  - |"),
            ("\t- tabbed|", "\t- tabbed\n\t- |"),
            ("# List\n\n- é|", "# List\n\n- é\n- |"),
            ("- one|\n- three", "- one\n- |\n- three"),
        ] {
            assert_eq!(enter(before), after, "for {before:?}");
        }
    }

    #[test]
    fn enter_inside_an_item_splits_it_in_two() {
        assert_eq!(enter("- milk |and eggs"), "- milk \n- |and eggs");
        assert_eq!(enter("- |milk"), "- \n- |milk");
    }

    #[test]
    fn enter_on_an_empty_item_ends_the_list() {
        assert_eq!(enter("- milk\n- |"), "- milk\n|");
        assert_eq!(enter("1. first\n2. |"), "1. first\n|");
        assert_eq!(enter("- [ ] call\n- [ ] |"), "- [ ] call\n|");
        assert_eq!(enter("- [ ]|"), "|");
        assert_eq!(enter("- a\n- |\nafter"), "- a\n|\nafter");
    }

    #[test]
    fn enter_on_an_empty_nested_item_goes_back_a_level_first() {
        assert_eq!(enter("- a\n  - b\n  - |"), "- a\n  - b\n- |");
        assert_eq!(enter("1. a\n   - b\n   - |"), "1. a\n   - b\n2. |");
        assert_eq!(
            enter("- [x] a\n    - b\n    - |"),
            "- [x] a\n    - b\n- [ ] |"
        );
        assert_eq!(enter("  - |"), "|");
        assert_eq!(enter("- a\n\n  - |"), "- a\n\n|");
    }

    #[test]
    fn enter_elsewhere_is_left_to_the_editor() {
        for text in [
            "plain|",
            "# Heading|",
            "-not a list|",
            "1.5 litres|",
            "1234567890. too long|",
            "---|",
            "* * *|",
            "- - -|",
            "**bold** start|",
            "```\n- code|",
            "~~~sh\n1. code|\n~~~",
            "|- at the start",
            "",
        ] {
            let text = if text.contains('|') {
                text.to_string()
            } else {
                format!("{text}|")
            };
            let expected = text.replacen('|', "\n|", 1);
            assert_eq!(enter(&text), expected, "for {text:?}");
        }
    }

    #[test]
    fn a_list_goes_on_after_a_code_block_ends() {
        assert_eq!(enter("```\ncode\n```\n- a|"), "```\ncode\n```\n- a\n- |");
    }

    #[test]
    fn only_a_fresh_line_gets_a_marker() {
        assert_eq!(after_enter("- a\nb", 5), None);
        assert_eq!(after_enter("- a", 3), None);
        assert_eq!(after_enter("", 0), None);
    }

    #[test]
    fn ticking_marks_a_task_done_and_back() {
        assert_eq!(toggle("- [ ] call|"), "- [x] call|");
        assert_eq!(toggle("- [x] call|"), "- [ ] call|");
        assert_eq!(toggle("- [X] call|"), "- [ ] call|");
        assert_eq!(toggle("1. [ ] |first"), "1. [x] |first");
        assert_eq!(toggle("a\n  * [ ] né|sted\nb"), "a\n  * [x] né|sted\nb");
        assert_eq!(toggle("|- [ ] call"), "|- [x] call");
        assert_eq!(toggle("- [|x] call"), "- [| ] call");
    }

    #[test]
    fn ticking_a_line_that_is_not_a_task_makes_it_one() {
        assert_eq!(toggle("- call|"), "- [ ] call|");
        assert_eq!(toggle("2. call|"), "2. [ ] call|");
        assert_eq!(toggle("call the plumber|"), "- [ ] call the plumber|");
        assert_eq!(toggle("  inden|ted"), "  - [ ] inden|ted");
        assert_eq!(toggle("|"), "- [ ] |");
        assert_eq!(toggle("- |"), "- [ ] |");
        assert_eq!(toggle("|call"), "- [ ] |call");
        assert_eq!(toggle("- [x]y|"), "- [ ] [x]y|", "a link, not a box");
    }

    #[test]
    fn code_is_never_ticked() {
        assert_eq!(toggle_task("```\n- [ ] code\n```", 8), None);
        assert_eq!(toggle_task("```sh", 2), None);
    }

    #[test]
    fn a_cursor_stays_with_its_text() {
        let edit = Edit {
            range: 4..6,
            text: "abcd".into(),
        };
        assert_eq!(edit.moved(2), 2, "before");
        assert_eq!(edit.moved(4), 4, "at the start");
        assert_eq!(edit.moved(5), 8, "inside");
        assert_eq!(edit.moved(6), 8, "at the end");
        assert_eq!(edit.moved(9), 11, "after");
    }
}
