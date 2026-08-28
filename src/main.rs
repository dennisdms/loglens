use iced::widget::{center, text};
use iced::{Element, Font};

pub fn main() -> iced::Result {
    iced::application(LogLens::default, LogLens::update, LogLens::view)
        .title("Log Lens")
        .run()
}

#[derive(Debug, Clone)]
enum Message {}

#[derive(Default)]
struct LogLens;

impl LogLens {
    fn update(&mut self, _message: Message) {}

    fn view(&self) -> Element<'_, Message> {
        center(text("Log Lens").size(48).font(Font::MONOSPACE)).into()
    }
}
