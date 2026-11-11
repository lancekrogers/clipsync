//! Terminal image preview with graphics protocols and a text-cell fallback.
use crate::control::ImagePreview;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use ratatui_image::{
    picker::{
        cap_parser::{Parser, QueryStdioOptions, Response},
        Picker, ProtocolType,
    },
    thread::{ResizeRequest, ResizeResponse, ThreadProtocol},
    StatefulImage,
};
use std::{
    io::Write,
    sync::mpsc,
    time::{Duration, Instant},
};
use uuid::Uuid;

// Query synchronously with poll: ratatui-image's timeout can leave a detached
// stdin reader that steals subsequent keystrokes when a terminal does not reply.
#[cfg(unix)]
fn probe_graphics() -> Option<Picker> {
    let tmux = std::env::var_os("TMUX").is_some();
    let query = Parser::query(tmux, QueryStdioOptions::default());
    std::io::stdout().write_all(query.as_bytes()).ok()?;
    std::io::stdout().flush().ok()?;
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut parser = Parser::new();
    let mut protocol = None;
    let mut cell = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let mut fd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut fd, 1, remaining.as_millis().max(1) as i32) } <= 0 {
            break;
        }
        let mut bytes = [0u8; 128];
        let n = unsafe { libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len()) };
        if n <= 0 {
            break;
        }
        let mut done = false;
        for byte in &bytes[..n as usize] {
            for response in parser.push(char::from(*byte)) {
                match response {
                    Response::Kitty => protocol = Some(ProtocolType::Kitty),
                    Response::Sixel if protocol.is_none() => protocol = Some(ProtocolType::Sixel),
                    Response::CellSize(Some((width, height))) if width > 0 && height > 0 => {
                        cell = Some((width, height))
                    }
                    Response::Status => done = true,
                    _ => {}
                }
            }
        }
        if done {
            break;
        }
    }
    let (width, height) = cell.unwrap_or((10, 20));
    // This constructor also preserves ratatui-image's tmux and iTerm detection.
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(width, height));
    if let Some(protocol) = protocol {
        picker.set_protocol_type(protocol);
    }
    Some(picker)
}

#[cfg(not(unix))]
fn probe_graphics() -> Option<Picker> {
    None
}

/// Terminal capabilities and image encoding are isolated from the controller.
pub struct GraphicsPreview {
    picker: Option<Picker>,
    state: ThreadProtocol,
    replies: mpsc::Receiver<ResizeResponse>,
    selected: Option<Uuid>,
    ready: bool,
    has_preview: bool,
}

impl GraphicsPreview {
    pub fn new(color: bool) -> Self {
        let picker = if color {
            probe_graphics().filter(|picker| picker.protocol_type() != ProtocolType::Halfblocks)
        } else {
            None
        };
        let (sender, requests) = mpsc::channel::<ResizeRequest>();
        let (reply, replies) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                if let Ok(result) = request.resize_encode() {
                    if reply.send(result).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            picker,
            state: ThreadProtocol::new(sender, None),
            replies,
            selected: None,
            ready: false,
            has_preview: false,
        }
    }

    pub fn update(&mut self, selected: Option<Uuid>, preview: Option<&ImagePreview>) -> bool {
        while let Ok(result) = self.replies.try_recv() {
            self.state.update_resized_protocol(result);
        }
        let available = preview.and_then(|p| p.png.as_ref());
        if self.selected == selected && self.has_preview == available.is_some() {
            return false;
        }
        let clear = self.ready;
        self.selected = selected;
        self.has_preview = available.is_some();
        self.ready = false;
        self.state.empty_protocol();
        if let (Some(picker), Some(decoded)) = (
            &self.picker,
            preview.and_then(crate::control::decode_preview_png),
        ) {
            if let Some(image) =
                image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
            {
                self.state.replace_protocol(
                    picker.new_resize_protocol(image::DynamicImage::ImageRgba8(image)),
                );
                self.ready = true;
            }
        }
        clear
    }

    pub fn render(&mut self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) -> bool {
        if !self.ready || area.width == 0 || area.height == 0 {
            return false;
        }
        frame.render_stateful_widget(StatefulImage::default(), area, &mut self.state);
        true
    }
}

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
            png: None,
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
            png: None,
        };
        assert!(lines(&invalid, 80, 40, true).is_empty());
    }
}
