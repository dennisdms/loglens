//! Symbolic SVG icons for the UI.
//!
//! Each icon is a single-path 16x16 SVG under `assets/icons/`, embedded into
//! the binary at build time. They carry a placeholder stroke colour; the `svg`
//! widget's colour filter recolours them to the palette at render time.

use std::sync::LazyLock;

use iced::widget::svg::Handle;

macro_rules! icon {
    ($name:ident => $file:literal) => {
        pub static $name: LazyLock<Handle> = LazyLock::new(|| {
            Handle::from_memory(include_bytes!(concat!("../assets/icons/", $file)).as_slice())
        });
    };
}

icon!(ARROW_LEFT => "arrow-left.svg");
icon!(ARROW_RIGHT => "arrow-right.svg");
icon!(TRASH => "trash.svg");
icon!(PLUS => "plus.svg");
icon!(SORT_FIELDS => "sort-fields.svg");
icon!(SORT_ASCENDING => "sort-ascending.svg");
icon!(SORT_DESCENDING => "sort-descending.svg");
icon!(SORT_REMOVE => "sort-remove.svg");
icon!(REFRESH => "refresh.svg");
icon!(WARNING => "warning-triangle.svg");
icon!(TABLE => "table.svg");
icon!(RAW_TEXT => "raw-text.svg");
icon!(HIGHLIGHT_RULES => "highlight-rules.svg");
icon!(FORMAT => "format.svg");

/// The window / taskbar icon: a magnifier over a dim log stream, one row in
/// focus. Decoded from the embedded 256x256 RGBA PNG at `assets/app-icon/
/// icon.png` (rendered from `icon.svg` beside it). Returns `None` if the PNG
/// can't be decoded into the 8-bit RGBA shape `iced` wants — the app then just
/// runs without an icon.
pub fn app_icon() -> Option<iced::window::Icon> {
    let bytes = include_bytes!("../assets/app-icon/icon.png");
    let mut reader = png::Decoder::new(bytes.as_slice()).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;

    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    iced::window::icon::from_rgba(buf, info.width, info.height).ok()
}

#[cfg(test)]
mod tests {
    /// The embedded app icon PNG must decode into the RGBA shape `iced` wants,
    /// so a bad re-export of `icon.png` fails the build rather than silently
    /// dropping the window icon.
    #[test]
    fn app_icon_decodes() {
        assert!(super::app_icon().is_some());
    }
}
