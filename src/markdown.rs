//! Markdown highlighting for the editor, a note's text without its Markdown, and
//! how many words it holds.
//! Each edit reparses the whole note, which is fine at note size. Colors are
//! resolved from the theme on every render, so they follow a theme change without
//! reparsing.

use gpui_kit::base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputEditorStyle, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::{
    Context, FontStyle, FontWeight, HighlightStyle, Hsla, SharedString, StrikethroughStyle,
    UnderlineStyle, Window, px,
};
use gpui_omarchy::Theme;
use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};
use std::{ops::Range, rc::Rc, sync::Arc};

pub const LANGUAGE: &str = "markdown";
/// The Markdown the editor shows as such, which is also what [`plain_text`] takes out.
const OPTIONS: Options = Options::ENABLE_STRIKETHROUGH.union(Options::ENABLE_TASKLISTS);

// Marks combine, so `***both***` gets bold and italic and `# **bold**` stays a
// heading. A mark's bit position is its index in `MARK_NAMES`.
const HEADING: u8 = 1 << 0;
const STRONG: u8 = 1 << 1;
const EMPHASIS: u8 = 1 << 2;
const CODE: u8 = 1 << 3;
const LINK: u8 = 1 << 4;
const QUOTE: u8 = 1 << 5;
const MARKER: u8 = 1 << 6;
const STRIKE: u8 = 1 << 7;
const MARK_NAMES: [&str; 8] = [
    "heading", "strong", "emphasis", "code", "link", "quote", "marker", "strike",
];

/// Ordered, non-overlapping styled ranges of `text`. Unstyled text has no run.
fn parse(text: &str) -> Vec<(Range<usize>, u8)> {
    let mut marks = vec![0u8; text.len()];
    for (event, range) in Parser::new_ext(text, OPTIONS).into_offset_iter() {
        let (mark, range) = match event {
            Event::Start(Tag::Heading { .. }) => (HEADING, range),
            Event::Start(Tag::Strong) => (STRONG, range),
            Event::Start(Tag::Emphasis) => (EMPHASIS, range),
            Event::Start(Tag::Strikethrough) => (STRIKE, range),
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => (LINK, range),
            Event::Start(Tag::CodeBlock(_)) | Event::Code(_) => (CODE, range),
            Event::Start(Tag::BlockQuote(_)) => (QUOTE, range),
            Event::TaskListMarker(_) | Event::Rule => (MARKER, range),
            // An item's range is the whole item; only its bullet is the marker.
            Event::Start(Tag::Item) => {
                let bullet = text[range.clone()]
                    .find(char::is_whitespace)
                    .unwrap_or(range.len());
                (MARKER, range.start..range.start + bullet)
            }
            _ => continue,
        };
        for byte in &mut marks[range] {
            *byte |= mark;
        }
    }

    let mut runs = Vec::new();
    let mut start = 0;
    for end in 1..=marks.len() {
        if end == marks.len() || marks[end] != marks[start] {
            if marks[start] != 0 {
                runs.push((start..end, marks[start]));
            }
            start = end;
        }
    }
    runs
}

/// The marks a piece of Markdown is written with, which [`plain_text`] takes out.
enum Marks {
    /// None to take out: a paragraph, a list, a quote.
    Kept,
    /// `**`, `*`, `~~` and the like on either side.
    Delimiters,
    /// The `#`s before, and any after, or the line of `=` or `-` under it.
    Heading,
    /// The lines of backticks around the code.
    Fences,
    /// The brackets, and the address unless it is given here to go after the text.
    Link(Option<String>),
}

/// A piece of Markdown being read, and where what is inside it starts and ends.
struct Open {
    marks: Marks,
    range: Range<usize>,
    inside: Option<Range<usize>>,
}

/// `text` without the Markdown that styles it, laid out as it was typed: the same
/// lines, blank lines and indentation. Headings, bold, italic, struck and code lose
/// their marks, and a link becomes its text followed by its address. Lists, tasks,
/// quotes and rules stay as they are, which is how plain text writes them too.
pub fn plain_text(text: &str) -> String {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    // Where the last thing read ended, to tell a `\` that escapes from one that's text.
    let mut read_to = 0;
    let mut in_code = false;
    for (event, range) in Parser::new_ext(text, OPTIONS).into_offset_iter() {
        if let Event::End(end) = event {
            in_code &= end != TagEnd::CodeBlock;
            if let Some(piece) = open.pop() {
                strip(text, piece, &mut edits);
            }
            continue;
        }
        if let Some(parent) = open.last_mut() {
            match &mut parent.inside {
                Some(inside) => inside.end = range.end,
                None => parent.inside = Some(range.clone()),
            }
        }
        match event {
            Event::Start(tag) => {
                in_code |= matches!(tag, Tag::CodeBlock(_));
                let marks = match tag {
                    Tag::Emphasis | Tag::Strong | Tag::Strikethrough => Marks::Delimiters,
                    Tag::Heading { .. } => Marks::Heading,
                    Tag::CodeBlock(CodeBlockKind::Fenced(_)) => Marks::Fences,
                    Tag::Link {
                        link_type,
                        dest_url,
                        ..
                    }
                    | Tag::Image {
                        link_type,
                        dest_url,
                        ..
                    } => Marks::Link(
                        // Elsewhere the address is the text itself, or stays in
                        // the note on a line of its own.
                        (link_type == LinkType::Inline && !dest_url.is_empty())
                            .then(|| dest_url.to_string()),
                    ),
                    _ => Marks::Kept,
                };
                open.push(Open {
                    marks,
                    range,
                    inside: None,
                });
                continue;
            }
            Event::Code(code) => edits.push((range.clone(), code.to_string())),
            Event::HardBreak => edits.push((range.clone(), "\n".into())),
            Event::Text(shown) if !in_code => {
                let typed = &text[range.clone()];
                let escaped = range.start > read_to
                    && text.as_bytes()[range.start - 1] == b'\\'
                    && typed.starts_with(|first: char| first.is_ascii_punctuation());
                if escaped {
                    edits.push((range.start - 1..range.start, String::new()));
                }
                // `&amp;` and its kind.
                if typed.starts_with('&') && *typed != *shown {
                    edits.push((range.clone(), shown.to_string()));
                }
            }
            _ => {}
        }
        read_to = range.end;
    }

    edits.sort_by_key(|(range, _)| range.start);
    let mut plain = String::with_capacity(text.len());
    let mut kept_to = 0;
    for (range, replacement) in edits {
        // A `\` before the first thing inside a piece is already among its marks,
        // and the wider edit, which comes first, stands.
        if range.start < kept_to {
            continue;
        }
        plain.push_str(&text[kept_to..range.start]);
        plain.push_str(&replacement);
        kept_to = range.end;
    }
    plain.push_str(&text[kept_to..]);
    plain
}

/// How many words someone reading `text` would count: what is written, without the
/// marks around it, the bullet, number or box of a list item, or a link's address.
/// Something with no letter or digit in it, such as a dash, isn't a word.
pub fn word_count(text: &str) -> usize {
    let mut count = 0;
    // The word being read has a letter or a digit in it.
    let mut counts = false;
    for event in Parser::new_ext(text, OPTIONS) {
        let read = match &event {
            Event::Text(read) | Event::Code(read) => read.as_ref(),
            // Styling can begin and end inside a word, which is still one word.
            Event::InlineHtml(_)
            | Event::Start(
                Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. },
            )
            | Event::End(
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image,
            ) => "",
            // Anything else ends a word: a new line, the next paragraph or item.
            _ => " ",
        };
        for character in read.chars() {
            if character.is_whitespace() {
                count += usize::from(counts);
                counts = false;
            } else {
                counts |= character.is_alphanumeric();
            }
        }
    }
    count + usize::from(counts)
}

/// Adds the edits that take `piece`'s own marks out, now that its extent is known.
fn strip(text: &str, piece: Open, edits: &mut Vec<(Range<usize>, String)>) {
    let Open {
        marks,
        range,
        inside,
    } = piece;
    let label = inside.clone().map_or("", |inside| &text[inside]);
    // With nothing inside, all of it is marks.
    let (before, after) = match inside {
        Some(inside) => (range.start..inside.start, inside.end..range.end),
        None => (range.start..range.start, range),
    };
    match marks {
        Marks::Kept => {}
        Marks::Delimiters => {
            edits.push((before, String::new()));
            edits.push((after, String::new()));
        }
        Marks::Heading => {
            edits.push((before, String::new()));
            // The heading's own line ending stays.
            let marks = text[after.clone()].trim_end_matches(['\n', '\r']);
            edits.push((after.start..after.start + marks.len(), String::new()));
        }
        Marks::Fences => {
            edits.push((before, String::new()));
            // The closing line goes whole, so no blank one is left in its place.
            let line_end = match &text[after.end..] {
                _ if after.is_empty() => 0,
                rest if rest.starts_with("\r\n") => 2,
                rest if rest.starts_with('\n') => 1,
                _ => 0,
            };
            edits.push((after.start..after.end + line_end, String::new()));
        }
        Marks::Link(address) => {
            let address = match address {
                Some(address) if label.is_empty() => address,
                Some(address) if address != label => format!(" ({address})"),
                _ => String::new(),
            };
            edits.push((before, String::new()));
            edits.push((after, address));
        }
    }
}

#[derive(Default)]
struct MarkdownHighlighter {
    runs: Vec<(Range<usize>, u8)>,
}

impl InputHighlighter for MarkdownHighlighter {
    fn language(&self) -> SharedString {
        LANGUAGE.into()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        self.runs = parse(&text.to_string());
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut styles = Vec::new();
        if range.is_empty() {
            return styles;
        }
        let mut position = range.start;
        let first = self.runs.partition_point(|(run, _)| run.end <= range.start);
        for (run, marks) in &self.runs[first..] {
            if run.start >= range.end {
                break;
            }
            let start = run.start.max(position);
            if start > position {
                styles.push((position..start, HighlightStyle::default()));
            }
            let end = run.end.min(range.end);
            styles.push((start..end, combined_style(*marks, resolver)));
            position = end;
        }
        if position < range.end {
            styles.push((position..range.end, HighlightStyle::default()));
        }
        styles
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

fn combined_style(marks: u8, resolver: &dyn HighlightStyleResolver) -> HighlightStyle {
    MARK_NAMES
        .iter()
        .enumerate()
        .filter(|(bit, _)| marks & (1 << bit) != 0)
        .filter_map(|(_, name)| resolver.style(name))
        .fold(HighlightStyle::default(), HighlightStyle::highlight)
}

pub fn highlighter_factory() -> InputHighlighterFactory {
    Rc::new(|_| Some(Box::new(MarkdownHighlighter::default())))
}

struct MarkdownStyles {
    accent: Hsla,
    secondary: Hsla,
    code: Hsla,
}

impl HighlightStyleResolver for MarkdownStyles {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        let colored = |color| HighlightStyle {
            color: Some(color),
            ..Default::default()
        };
        Some(match name {
            "heading" => HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..colored(self.accent)
            },
            "strong" => HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
            "emphasis" => HighlightStyle {
                font_style: Some(FontStyle::Italic),
                ..Default::default()
            },
            "code" => colored(self.code),
            "link" => HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(self.accent),
                    wavy: false,
                }),
                ..colored(self.accent)
            },
            "quote" | "marker" => colored(self.secondary),
            "strike" => HighlightStyle {
                strikethrough: Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(self.secondary),
                }),
                ..colored(self.secondary)
            },
            _ => return None,
        })
    }
}

/// The editor's colors for the current theme, including markdown styles.
pub fn editor_style(theme: &Theme) -> InputEditorStyle {
    InputEditorStyle {
        highlight_styles: Arc::new(MarkdownStyles {
            accent: theme.accent,
            secondary: theme.secondary,
            code: theme.success,
        }),
        ..theme.input_style()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text covered by each mark, in order, for readable expectations.
    fn marked(text: &str, mark: u8) -> Vec<&str> {
        parse(text)
            .into_iter()
            .filter(|(_, marks)| marks & mark != 0)
            .map(|(range, _)| &text[range])
            .collect()
    }

    struct EveryMarkStyled;
    impl HighlightStyleResolver for EveryMarkStyled {
        fn style(&self, _: &str) -> Option<HighlightStyle> {
            Some(HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            })
        }
    }

    #[test]
    fn recognizes_the_common_markdown_elements() {
        assert_eq!(marked("# Title\n", HEADING), ["# Title\n"]);
        assert_eq!(marked("a **b** c", STRONG), ["**b**"]);
        assert_eq!(marked("a *b* c", EMPHASIS), ["*b*"]);
        assert_eq!(marked("a ~~b~~ c", STRIKE), ["~~b~~"]);
        assert_eq!(marked("a `b` c", CODE), ["`b`"]);
        assert_eq!(marked("```\ncode\n```\n", CODE), ["```\ncode\n```"]);
        assert_eq!(marked("[a](http://b)", LINK), ["[a](http://b)"]);
        assert_eq!(marked("> quote\n", QUOTE), ["> quote\n"]);
        assert_eq!(marked("---\n", MARKER), ["---\n"]);
    }

    #[test]
    fn only_the_bullet_of_a_list_item_is_a_marker() {
        assert_eq!(marked("- one\n- two\n", MARKER), ["-", "-"]);
        assert_eq!(marked("1. one\n", MARKER), ["1."]);
        assert_eq!(marked("- [x] done\n", MARKER), ["-", "[x]"]);
    }

    #[test]
    fn nested_marks_combine() {
        let runs = parse("***both***");
        assert!(
            runs.iter()
                .any(|(_, m)| m & STRONG != 0 && m & EMPHASIS != 0)
        );
        let runs = parse("# a **b**\n");
        assert!(
            runs.iter()
                .any(|(_, m)| m & STRONG != 0 && m & HEADING != 0)
        );
    }

    #[test]
    fn plain_text_drops_the_marks_and_keeps_the_words() {
        assert_eq!(plain_text("# Title\n"), "Title\n");
        assert_eq!(plain_text("## Title ##\nmore"), "Title\nmore");
        assert_eq!(plain_text("Title\n=====\nmore"), "Title\nmore");
        assert_eq!(plain_text("#\nmore"), "\nmore");
        assert_eq!(
            plain_text("a **b** *c* _d_ __e__ ~~f~~ `g` h"),
            "a b c d e f g h"
        );
        assert_eq!(plain_text("***both*** and # **bold**"), "both and # bold");
        assert_eq!(plain_text("`` a`b ``"), "a`b");
        assert_eq!(
            plain_text("1 * 2 * 3 and snake_case_name"),
            "1 * 2 * 3 and snake_case_name"
        );
    }

    #[test]
    fn plain_text_keeps_the_layout_as_typed() {
        let note = "First line\nsecond line\n\n\n  indented\n\nlast, no newline";
        assert_eq!(plain_text(note), note);
        assert_eq!(plain_text(""), "");
        assert_eq!(
            plain_text("two spaces  \nor a slash\\\nend"),
            "two spaces\nor a slash\nend"
        );
    }

    #[test]
    fn plain_text_leaves_lists_quotes_and_rules_alone() {
        let note = "- one\n  - [x] done\n  - [ ] to do\n* star\n\n1. first\n2. second\n\n\
                    > quoted\n> again\n\n---\n";
        assert_eq!(plain_text(note), note);
        assert_eq!(
            plain_text("- **bold** item\n> *quoted*\n"),
            "- bold item\n> quoted\n"
        );
    }

    #[test]
    fn plain_text_gives_a_link_its_address() {
        assert_eq!(
            plain_text("see [the site](https://omarchy.org) now"),
            "see the site (https://omarchy.org) now"
        );
        assert_eq!(
            plain_text("[**bold** link](http://a \"title\")"),
            "bold link (http://a)"
        );
        assert_eq!(plain_text("![a picture](cat.png)"), "a picture (cat.png)");
        assert_eq!(plain_text("[](http://a) and [b]()"), "http://a and b");
        assert_eq!(plain_text("[http://a](http://a)"), "http://a");
        assert_eq!(
            plain_text("<http://a> or <me@example.org>"),
            "http://a or me@example.org"
        );
        // The address stays where it was written.
        assert_eq!(
            plain_text("[a][ref]\n\n[ref]: http://a\n"),
            "a\n\n[ref]: http://a\n"
        );
        assert_eq!(plain_text("[not a link] and [x]"), "[not a link] and [x]");
    }

    #[test]
    fn plain_text_takes_the_fences_off_code_and_leaves_what_is_inside() {
        assert_eq!(
            plain_text("```\n# not **marks**\n```\n"),
            "# not **marks**\n"
        );
        assert_eq!(
            plain_text("a\n\n```rust\nlet x = 1;\n\nlet y = 2;\n```\n\nb\n"),
            "a\n\nlet x = 1;\n\nlet y = 2;\n\nb\n"
        );
        assert_eq!(plain_text("a\n\n```\n```\n\nb"), "a\n\n\nb");
        assert_eq!(
            plain_text("```\nnever closed *é*\nstill code"),
            "never closed *é*\nstill code"
        );
        assert_eq!(
            plain_text("- a\n  ```\n  code\n  ```\n- b\n"),
            "- a\n  code\n- b\n"
        );
        assert_eq!(
            plain_text("> ```\n> code\n> ```\n> after\n"),
            "> code\n> after\n"
        );
        assert_eq!(plain_text("    indented *code*\n"), "    indented *code*\n");
        assert_eq!(
            plain_text("`*code*` and \\`not code\\`"),
            "*code* and `not code`"
        );
    }

    #[test]
    fn plain_text_resolves_escapes_and_entities() {
        assert_eq!(plain_text("2 \\* 3 \\# 4, **\\*a**"), "2 * 3 # 4, *a");
        assert_eq!(plain_text("a\\b and C:\\dir"), "a\\b and C:\\dir");
        assert_eq!(plain_text("back\\\\slash"), "back\\slash");
        assert_eq!(
            plain_text("AT&amp;T &copy; R&D &nope;"),
            "AT&T © R&D &nope;"
        );
        assert_eq!(plain_text("<b>html</b> stays"), "<b>html</b> stays");
    }

    /// Whatever is typed, nothing is cut inside a character, and text with no
    /// Markdown in it comes back as it was.
    #[test]
    fn plain_text_copes_with_unfinished_markdown() {
        for text in [
            "# Héading é\n\nwörd **grâs** *ital* `còde` ~~bàr~~\n",
            "- à\n- [x] é\n\n> çitation\n\n[lïen](http://x)\n",
            "***\n**unterminated *nest\n`open\n",
            "日本語 **太字** テキスト\n\n# 見出し\n",
            "[a](\n**\\\n# \n```",
            "\\",
            "&",
        ] {
            for (end, _) in text.char_indices().chain([(text.len(), ' ')]) {
                plain_text(&text[..end]);
            }
        }
        assert_eq!(
            plain_text("**unterminated *nest\n`open\n"),
            "**unterminated *nest\n`open\n"
        );
    }

    #[test]
    fn words_are_counted_as_someone_reading_the_note_would() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("  \n\n "), 0);
        assert_eq!(word_count("one"), 1);
        assert_eq!(word_count("one two\nthree\n\nfour  five"), 5);
        assert_eq!(word_count("l'été à 9 h, c'est-à-dire tôt"), 6);
        assert_eq!(word_count("日本語 テキスト"), 2);
        assert_eq!(word_count("wait — what? ... & so on"), 4);
    }

    #[test]
    fn markdown_itself_is_not_counted() {
        assert_eq!(word_count("# Title\n\n## Another one ##\n"), 3);
        assert_eq!(
            word_count("- one\n- two\n  - three\n\n1. four\n2. five\n"),
            5
        );
        assert_eq!(word_count("- [x] done\n- [ ] to do\n"), 3);
        assert_eq!(word_count("> quoted words\n\n---\n\nafter"), 3);
        assert_eq!(word_count("see [the site](https://omarchy.org/a/b) now"), 4);
        assert_eq!(word_count("```rust\nlet x = 1;\n```\n"), 3);
        assert_eq!(word_count("some `inline code` here"), 4);
    }

    #[test]
    fn styling_inside_a_word_leaves_it_one_word() {
        assert_eq!(word_count("un**believ**able *it*self ~~no~~pe"), 3);
        assert_eq!(word_count("AT&amp;T and a\\*b"), 3);
        assert_eq!(word_count("**bold** *italic*"), 2);
        assert_eq!(word_count("end of one\n**start** of the next"), 7);
    }

    #[test]
    fn plain_text_and_empty_notes_have_no_runs() {
        assert!(parse("").is_empty());
        assert!(parse("just some words\n").is_empty());
    }

    /// Exercises the contract the editor relies on: runs are ordered, do not
    /// overlap, fully cover the requested range and land on char boundaries.
    #[test]
    fn styles_fully_cover_any_range_in_order() {
        for text in [
            "",
            "plain",
            "# Héading é\n\nwörd **grâs** *ital* `còde` ~~bàr~~\n",
            "- à\n- [x] é\n\n> çitation\n\n[lïen](http://x)\n",
            "```\nnever closed é\nstill code",
            "***\n**unterminated *nest\n`open\n",
            "日本語 **太字** テキスト\n\n# 見出し\n",
        ] {
            let highlighter = MarkdownHighlighter { runs: parse(text) };
            let boundaries: Vec<_> = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .collect();
            for &start in &boundaries {
                for &end in boundaries.iter().filter(|&&end| end >= start) {
                    let range = start..end;
                    let styles = highlighter.styles(&range, &EveryMarkStyled);
                    let mut position = start;
                    for (run, _) in &styles {
                        assert_eq!(run.start, position, "gap or overlap in {text:?} {range:?}");
                        assert!(run.end > run.start, "empty run in {text:?} {range:?}");
                        assert!(text.is_char_boundary(run.start), "{text:?} {run:?}");
                        assert!(text.is_char_boundary(run.end), "{text:?} {run:?}");
                        position = run.end;
                    }
                    assert_eq!(position, end, "incomplete cover in {text:?} {range:?}");
                }
            }
        }
    }

    #[test]
    fn unstyled_text_between_runs_gets_the_default_style() {
        let highlighter = MarkdownHighlighter {
            runs: parse("a **b** c"),
        };
        let styles = highlighter.styles(&(0..9), &EveryMarkStyled);
        assert_eq!(styles.len(), 3);
        assert_eq!(styles[0], (0..2, HighlightStyle::default()));
        assert_eq!(styles[1].0, 2..7);
        assert_ne!(styles[1].1, HighlightStyle::default());
        assert_eq!(styles[2], (7..9, HighlightStyle::default()));
    }
}
