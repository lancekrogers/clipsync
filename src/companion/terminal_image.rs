//! Terminal image preview using ordinary colored text cells.
use crate::control::ImagePreview;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// Render two source pixels in each cell. This uses no terminal-specific image
/// escape sequences, and falls back to grayscale characters with NO_COLOR.
pub fn lines(preview: &ImagePreview, width: u16, height: u16, color: bool) -> Vec<Line<'static>> {
    if !preview.is_valid() || width == 0 || height == 0 {
        return Vec::new();
    }
    let max_width = u32::from(width).min(120);
    let max_height = (u32::from(height) * 2).min(120);
    let out_width = max_width.min((max_height * preview.width / preview.height).max(1));
    let out_height = (out_width * preview.height / preview.width)
        .max(1)
        .min(max_height);
    let pixel = |x: u32, y: u32| -> [u8; 3] {
        if y >= out_height {
            return [24, 24, 24];
        }
        let source_x = x * preview.width / out_width;
        let source_y = y * preview.height / out_height;
        let offset = ((source_y * preview.width + source_x) * 3) as usize;
        [
            preview.rgb[offset],
            preview.rgb[offset + 1],
            preview.rgb[offset + 2],
        ]
    };
    (0..out_height.div_ceil(2))
        .map(|row| {
            let spans = (0..out_width)
                .map(|x| {
                    let top = pixel(x, row * 2);
                    let bottom = pixel(x, row * 2 + 1);
                    if color {
                        Span::styled(
                            "▀",
                            Style::default()
                                .fg(Color::Rgb(top[0], top[1], top[2]))
                                .bg(Color::Rgb(bottom[0], bottom[1], bottom[2])),
                        )
                    } else {
                        let sum: u32 = top.iter().chain(bottom.iter()).map(|v| u32::from(*v)).sum();
                        let index = (sum * 9 / (6 * 255)) as usize;
                        Span::raw(char::from(b" .:-=+*#%@"[index]).to_string())
                    }
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_color_and_monochrome_and_rejects_invalid_input() {
        let image = ImagePreview {
            width: 1,
            height: 2,
            rgb: vec![255, 0, 0, 0, 0, 255],
        };
        let rendered = lines(&image, 1, 1, true);
        assert_eq!(rendered.len(), 1);
        assert_eq!(rendered[0].spans[0].content, "▀");
        assert_eq!(rendered[0].spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(rendered[0].spans[0].style.bg, Some(Color::Rgb(0, 0, 255)));
        assert_eq!(lines(&image, 1, 1, false)[0].spans.len(), 1);
        let invalid = ImagePreview {
            width: 1,
            height: 2,
            rgb: vec![0],
        };
        assert!(lines(&invalid, 80, 40, true).is_empty());
    }
}
