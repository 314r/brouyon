//! Markdown highlighting for the editor. Each edit reparses the whole note, which
//! is fine at note size. Colors are resolved from the theme on every render, so
//! they follow a theme change without reparsing.

use gpui_kit::base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputEditorStyle, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::{
    Context, FontStyle, FontWeight, HighlightStyle, Hsla, SharedString, StrikethroughStyle,
    UnderlineStyle, Window, px,
};
use gpui_omarchy::Theme;
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::{ops::Range, rc::Rc, sync::Arc};

pub const LANGUAGE: &str = "markdown";

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
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
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
