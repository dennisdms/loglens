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
icon!(SORT_ASCENDING => "sort-ascending.svg");
icon!(SORT_DESCENDING => "sort-descending.svg");
icon!(SORT_REMOVE => "sort-remove.svg");
