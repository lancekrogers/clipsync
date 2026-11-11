//! Semantic styles for the terminal companion. Colors degrade from 24-bit to
//! the 256- and 16-color palettes; NO_COLOR keeps only weight, reverse video
//! and symbols.
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders},
};
use unicode_width::UnicodeWidthStr;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    Mono,
    Basic,
    Indexed,
    Rgb,
}

impl Depth {
    pub fn detect(no_color: bool, colorterm: Option<&str>, term: Option<&str>) -> Self {
        if no_color {
            return Self::Mono;
        }
        let colorterm = colorterm.unwrap_or_default().to_ascii_lowercase();
        let term = term.unwrap_or_default();
        if matches!(colorterm.as_str(), "truecolor" | "24bit") || term.ends_with("-direct") {
            Self::Rgb
        } else if [
            "256color",
            "kitty",
            "ghostty",
            "wezterm",
            "alacritty",
            "foot",
        ]
        .iter()
        .any(|name| term.contains(name))
        {
            Self::Indexed
        } else {
            Self::Basic
        }
    }

    pub fn from_env() -> Self {
        Self::detect(
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    }
}

/// What a status, notice or value means, independent of how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
    Info,
    Quiet,
}

#[derive(Clone, Copy)]
struct Palette {
    accent: Color,
    success: Color,
    warning: Color,
    danger: Color,
    muted: Color,
    border: Color,
    /// Selected rows set both colors so they stay legible on light terminals.
    selection: Option<(Color, Color)>,
    ink: Color,
    hues: [Color; 6],
}

const RGB: Palette = Palette {
    accent: Color::Rgb(105, 191, 255),
    success: Color::Rgb(122, 214, 140),
    warning: Color::Rgb(246, 193, 92),
    danger: Color::Rgb(244, 112, 112),
    muted: Color::Rgb(134, 142, 160),
    border: Color::Rgb(74, 82, 100),
    selection: Some((Color::Rgb(31, 43, 62), Color::Rgb(236, 241, 248))),
    ink: Color::Rgb(14, 18, 26),
    hues: [
        Color::Rgb(94, 214, 196),
        Color::Rgb(196, 160, 255),
        Color::Rgb(255, 162, 112),
        Color::Rgb(250, 138, 190),
        Color::Rgb(178, 218, 105),
        Color::Rgb(140, 172, 255),
    ],
};

const INDEXED: Palette = Palette {
    accent: Color::Indexed(75),
    success: Color::Indexed(114),
    warning: Color::Indexed(221),
    danger: Color::Indexed(203),
    muted: Color::Indexed(245),
    border: Color::Indexed(240),
    selection: Some((Color::Indexed(236), Color::Indexed(255))),
    ink: Color::Indexed(16),
    hues: [
        Color::Indexed(79),
        Color::Indexed(183),
        Color::Indexed(216),
        Color::Indexed(212),
        Color::Indexed(149),
        Color::Indexed(111),
    ],
};

// The 16 named colors follow the user's terminal theme, so avoid backgrounds.
const BASIC: Palette = Palette {
    accent: Color::LightBlue,
    success: Color::Green,
    warning: Color::Yellow,
    danger: Color::Red,
    muted: Color::DarkGray,
    border: Color::DarkGray,
    selection: None,
    ink: Color::Black,
    hues: [
        Color::Cyan,
        Color::Magenta,
        Color::LightRed,
        Color::LightMagenta,
        Color::LightGreen,
        Color::LightCyan,
    ],
};

#[derive(Clone, Copy)]
pub struct Theme {
    depth: Depth,
    palette: Palette,
}

impl Theme {
    pub fn new(depth: Depth) -> Self {
        let palette = match depth {
            Depth::Rgb => RGB,
            Depth::Indexed => INDEXED,
            Depth::Basic | Depth::Mono => BASIC,
        };
        Self { depth, palette }
    }

    pub fn color(&self) -> bool {
        self.depth != Depth::Mono
    }

    fn fg(&self, color: Color) -> Style {
        if self.color() {
            Style::default().fg(color)
        } else {
            Style::default()
        }
    }

    pub fn accent(&self) -> Style {
        if self.color() {
            self.fg(self.palette.accent)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    pub fn heading(&self) -> Style {
        self.accent().add_modifier(Modifier::BOLD)
    }

    pub fn key(&self) -> Style {
        self.heading()
    }

    pub fn muted(&self) -> Style {
        self.fg(self.palette.muted)
    }

    pub fn link(&self) -> Style {
        self.fg(self.palette.accent)
            .add_modifier(Modifier::UNDERLINED)
    }

    pub fn tone(&self, tone: Tone) -> Style {
        let color = match tone {
            Tone::Good => self.palette.success,
            Tone::Warn => self.palette.warning,
            Tone::Bad => self.palette.danger,
            Tone::Info => self.palette.accent,
            Tone::Quiet => return self.muted(),
        };
        self.fg(color).add_modifier(Modifier::BOLD)
    }

    /// A filled label that stays visible at a glance, even without color.
    pub fn badge(&self, tone: Tone) -> Style {
        let fill = match tone {
            Tone::Good => self.palette.success,
            Tone::Warn => self.palette.warning,
            Tone::Bad => self.palette.danger,
            Tone::Info => self.palette.accent,
            Tone::Quiet => self.palette.muted,
        };
        let style = Style::default().add_modifier(Modifier::BOLD);
        if self.color() {
            style.fg(self.palette.ink).bg(fill)
        } else {
            style.add_modifier(Modifier::REVERSED)
        }
    }

    pub fn tab(&self, active: bool) -> Style {
        if active {
            self.badge(Tone::Info)
        } else {
            self.muted()
        }
    }

    pub fn border(&self, focused: bool) -> Style {
        if focused {
            self.accent()
        } else {
            self.fg(self.palette.border)
        }
    }

    pub fn selection(&self) -> Style {
        match (self.color(), self.palette.selection) {
            (true, Some((bg, fg))) => Style::default().bg(bg).fg(fg),
            _ => Style::default(),
        }
    }

    pub fn selection_bar(&self) -> Span<'static> {
        if self.color() {
            Span::styled("▌", self.accent())
        } else {
            Span::styled(">", Style::default().add_modifier(Modifier::BOLD))
        }
    }

    /// A stable hue per device id so the same computer always reads the same.
    pub fn device(&self, id: Uuid) -> Style {
        let hash = id.as_bytes().iter().fold(0x811c_9dc5u32, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
        });
        self.fg(self.palette.hues[hash as usize % self.palette.hues.len()])
    }

    /// The terminal's own foreground. Titles need it explicitly or they
    /// inherit the border color underneath them.
    pub fn text(&self) -> Style {
        if self.color() {
            Style::default().fg(Color::Reset)
        } else {
            Style::default()
        }
    }

    pub fn title(&self) -> Style {
        self.text().add_modifier(Modifier::BOLD)
    }

    pub fn frame<'a>(&self, focused: bool) -> Block<'a> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.border(focused))
            .title_style(self.text())
    }

    /// A pane with an accent heading; only the focused pane gets an accent border.
    pub fn block<'a>(&self, title: &str, focused: bool) -> Block<'a> {
        self.frame(focused)
            .title(Span::styled(title.to_owned(), self.heading()))
    }

    /// Key hints as `key label` pairs. Pairs that do not fit are dropped from
    /// the middle so the final pair (usually quit) stays visible.
    pub fn hints(&self, pairs: &[(&str, &str)], width: u16) -> Line<'static> {
        let spans_for = |(key, label): &(&str, &str)| {
            vec![
                Span::styled(key.to_string(), self.key()),
                Span::styled(format!(" {label}"), self.muted()),
            ]
        };
        let cost = |pair: &(&str, &str)| pair.0.width() + 1 + pair.1.width() + 2;
        let budget = usize::from(width).saturating_sub(1);
        let (last, rest) = match pairs.split_last() {
            Some(split) => split,
            None => return Line::default(),
        };
        let mut used = cost(last);
        let mut spans = vec![Span::raw(" ")];
        for pair in rest {
            if used + cost(pair) > budget {
                break;
            }
            used += cost(pair);
            spans.extend(spans_for(pair));
            spans.push(Span::raw("  "));
        }
        spans.extend(spans_for(last));
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_prefers_no_color_then_truecolor_then_256() {
        assert_eq!(
            Depth::detect(true, Some("truecolor"), Some("xterm-256color")),
            Depth::Mono
        );
        assert_eq!(
            Depth::detect(false, Some("24bit"), Some("xterm")),
            Depth::Rgb
        );
        assert_eq!(
            Depth::detect(false, None, Some("xterm-256color")),
            Depth::Indexed
        );
        assert_eq!(Depth::detect(false, None, Some("xterm")), Depth::Basic);
        assert_eq!(Depth::detect(false, None, None), Depth::Basic);
    }

    #[test]
    fn monochrome_uses_no_colors_but_keeps_emphasis() {
        let theme = Theme::new(Depth::Mono);
        for style in [
            theme.accent(),
            theme.muted(),
            theme.tone(Tone::Bad),
            theme.badge(Tone::Warn),
            theme.selection(),
            theme.device(Uuid::new_v4()),
            theme.border(true),
        ] {
            assert_eq!((style.fg, style.bg), (None, None));
        }
        assert!(theme
            .badge(Tone::Bad)
            .add_modifier
            .contains(Modifier::REVERSED));
        assert_eq!(theme.selection_bar().content, ">");
    }

    #[test]
    fn device_hues_are_stable_and_selection_sets_both_colors() {
        let theme = Theme::new(Depth::Rgb);
        let id = Uuid::from_u128(42);
        assert_eq!(theme.device(id), theme.device(id));
        let selection = theme.selection();
        assert!(selection.fg.is_some() && selection.bg.is_some());
        assert_eq!(Theme::new(Depth::Basic).selection(), Style::default());
    }

    #[test]
    fn hints_keep_the_last_pair_when_narrow() {
        let theme = Theme::new(Depth::Mono);
        let pairs = [("1-4", "sections"), ("Tab", "next"), ("q", "quit")];
        let text = |line: Line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        assert_eq!(
            text(theme.hints(&pairs, 80)),
            " 1-4 sections  Tab next  q quit"
        );
        assert_eq!(text(theme.hints(&pairs, 24)), " 1-4 sections  q quit");
        assert_eq!(text(theme.hints(&pairs, 10)), " q quit");
    }
}
