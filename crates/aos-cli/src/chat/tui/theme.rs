use ratatui::style::{Color, Style};

pub(crate) const COMPOSER_BG: Color = Color::Rgb(58, 63, 72);

pub(crate) fn composer_band_style() -> Style {
    Style::default().bg(COMPOSER_BG)
}
