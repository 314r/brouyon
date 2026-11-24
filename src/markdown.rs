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
const OPTIONS: Options = Options::ENABLE_STRIKETHROUGH.union(Options::ENABLE_TASKLISTS);

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

enum Marks {
    Kept,
    Delimiters,
    Heading,
    Fences,
    Link(Option<String>),
}

struct Open {
    marks: Marks,
    range: Range<usize>,
    inside: Option<Range<usize>>,
}

pub fn plain_text(text: &str) -> String {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut open: Vec<Open> = Vec::new();
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

pub fn word_count(text: &str) -> usize {
    let mut count = 0;
    let mut counts = false;
    for event in Parser::new_ext(text, OPTIONS) {
        let read = match &event {
            Event::Text(read) | Event::Code(read) => read.as_ref(),
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

fn strip(text: &str, piece: Open, edits: &mut Vec<(Range<usize>, String)>) {
    let Open {
        marks,
        range,
        inside,
    } = piece;
    let label = inside.clone().map_or("", |inside| &text[inside]);
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
            let marks = text[after.clone()].trim_end_matches(['\n', '\r']);
            edits.push((after.start..after.start + marks.len(), String::new()));
        }
        Marks::Fences => {
            edits.push((before, String::new()));
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
    use gpui_kit::{AppContext, TestAppContext, VisualTestContext};

    fn highlighter(text: &str, cx: &mut VisualTestContext) -> Box<dyn InputHighlighter> {
        let mut highlighter = highlighter_factory()(LANGUAGE).unwrap();
        cx.update(|window, cx| {
            let editor = cx.new(|cx| EditorState::new(window, cx));
            editor.update(cx, |_, cx| {
                highlighter.update(None, &Rope::from(text), false, window, cx);
            });
        });
        highlighter
    }

    fn styles(
        highlighter: &dyn InputHighlighter,
        range: &Range<usize>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let editor = editor_style(&Theme::tokyo_night());
        highlighter.styles(range, &*editor.highlight_styles)
    }

    fn styled<'a>(text: &'a str, cx: &mut VisualTestContext) -> Vec<(&'a str, HighlightStyle)> {
        styles(&*highlighter(text, cx), &(0..text.len()))
            .into_iter()
            .map(|(range, style)| (&text[range], style))
            .collect()
    }

    fn colored(color: Hsla) -> HighlightStyle {
        HighlightStyle {
            color: Some(color),
            ..Default::default()
        }
    }

    #[gpui_kit::test]
    fn recognizes_the_common_markdown_elements(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let t = Theme::tokyo_night();
        let plain = HighlightStyle::default();
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..plain
        };
        let italic = HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..plain
        };
        let heading = bold.highlight(colored(t.accent));
        let struck = HighlightStyle {
            strikethrough: Some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(t.secondary),
            }),
            ..colored(t.secondary)
        };
        let link = HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(t.accent),
                wavy: false,
            }),
            ..colored(t.accent)
        };
        for (piece, style) in [
            ("# Title\n", heading),
            ("```\ncode\n```", colored(t.success)),
            ("[a](http://b)", link),
            ("> quote\n", colored(t.secondary)),
            ("---\n", colored(t.secondary)),
            ("just some words\n", plain),
        ] {
            assert_eq!(styled(piece, cx), [(piece, style)]);
        }
        for (piece, style) in [
            ("**b**", bold),
            ("*b*", italic),
            ("~~b~~", struck),
            ("`b`", colored(t.success)),
        ] {
            let text = format!("a {piece} c");
            let expected = [("a ", plain), (piece, style), (" c", plain)];
            assert_eq!(styled(&text, cx), expected);
        }
        assert!(styled("", cx).is_empty());
    }

    #[gpui_kit::test]
    fn only_the_bullet_of_a_list_item_is_a_marker(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let marker = colored(Theme::tokyo_night().secondary);
        for (text, markers) in [
            ("- one\n- two\n", vec!["-", "-"]),
            ("1. one\n", vec!["1."]),
            ("- [x] done\n", vec!["-", "[x]"]),
        ] {
            let mut pieces = styled(text, cx);
            pieces.retain(|(_, style)| *style != HighlightStyle::default());
            let expected: Vec<_> = markers.into_iter().map(|piece| (piece, marker)).collect();
            assert_eq!(pieces, expected);
        }
    }

    #[gpui_kit::test]
    fn nested_marks_combine(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let heading = bold.highlight(colored(Theme::tokyo_night().accent));
        let italic = |style| HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..style
        };
        let mut shown = |text, style| styled(text, cx).iter().any(|piece| piece.1 == style);
        assert!(shown("***both***", italic(bold)));
        assert!(shown("# a *b*\n", italic(heading)));
        let in_heading = styled("# a **b**\n", cx);
        assert!(in_heading.iter().all(|piece| piece.1 == heading));
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

    #[gpui_kit::test]
    fn styles_fully_cover_any_range_in_order(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        for text in [
            "",
            "plain",
            "# Héading é\n\nwörd **grâs** *ital* `còde` ~~bàr~~\n",
            "- à\n- [x] é\n\n> çitation\n\n[lïen](http://x)\n",
            "```\nnever closed é\nstill code",
            "***\n**unterminated *nest\n`open\n",
            "日本語 **太字** テキスト\n\n# 見出し\n",
        ] {
            let highlighter = highlighter(text, cx);
            let boundaries: Vec<_> = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .collect();
            for &start in &boundaries {
                for &end in boundaries.iter().filter(|&&end| end >= start) {
                    let range = start..end;
                    let styles = styles(&*highlighter, &range);
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
}
