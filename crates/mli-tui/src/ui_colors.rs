use std::sync::OnceLock;

use ratatui::style::Color;

use crate::color::is_light;
use crate::terminal_palette::best_color;
use crate::terminal_palette::default_bg;
use crate::terminal_palette::default_fg;

pub const SECONDARY_LIGHT_RGB: &str = "#7B5BB6";
pub const SECONDARY_DARK_RGB: &str = "#F694FF";

pub const ORANGE_LIGHT_RGB: &str = "#D68C27";
pub const ORANGE_DARK_RGB: &str = "#F5A742";

#[derive(Clone, Copy)]
struct UiRgbConstants {
    secondary_light: (u8, u8, u8),
    secondary_dark: (u8, u8, u8),
    orange_light: (u8, u8, u8),
    orange_dark: (u8, u8, u8),
}

fn ui_rgb_constants() -> &'static UiRgbConstants {
    static CONSTANTS: OnceLock<UiRgbConstants> = OnceLock::new();
    CONSTANTS.get_or_init(|| UiRgbConstants {
        secondary_light: parse_hex_rgb(SECONDARY_LIGHT_RGB).unwrap_or_else(|| {
            panic!("SECONDARY_LIGHT_RGB must be #RRGGBB: {SECONDARY_LIGHT_RGB}")
        }),
        secondary_dark: parse_hex_rgb(SECONDARY_DARK_RGB)
            .unwrap_or_else(|| panic!("SECONDARY_DARK_RGB must be #RRGGBB: {SECONDARY_DARK_RGB}")),
        orange_light: parse_hex_rgb(ORANGE_LIGHT_RGB)
            .unwrap_or_else(|| panic!("ORANGE_LIGHT_RGB must be #RRGGBB: {ORANGE_LIGHT_RGB}")),
        orange_dark: parse_hex_rgb(ORANGE_DARK_RGB)
            .unwrap_or_else(|| panic!("ORANGE_DARK_RGB must be #RRGGBB: {ORANGE_DARK_RGB}")),
    })
}

fn parse_hex_rgb(value: &str) -> Option<(u8, u8, u8)> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&value[0..2], 16).ok()?;
    let g = u8::from_str_radix(&value[2..4], 16).ok()?;
    let b = u8::from_str_radix(&value[4..6], 16).ok()?;
    Some((r, g, b))
}

fn choose_rgb_for_theme(
    light: (u8, u8, u8),
    dark: (u8, u8, u8),
    terminal_bg: Option<(u8, u8, u8)>,
    terminal_fg: Option<(u8, u8, u8)>,
) -> (u8, u8, u8) {
    if let Some(bg) = terminal_bg {
        if is_light(bg) {
            return light;
        }
        return dark;
    }

    if let Some(fg) = terminal_fg {
        // If the foreground is light, assume a dark background.
        if is_light(fg) {
            return dark;
        }
        return light;
    }

    // Default to dark-mode palettes when theme detection isn't available.
    dark
}

pub fn secondary_color() -> Color {
    let constants = ui_rgb_constants();
    best_color(choose_rgb_for_theme(
        constants.secondary_light,
        constants.secondary_dark,
        default_bg(),
        default_fg(),
    ))
}

pub fn orange_color() -> Color {
    let constants = ui_rgb_constants();
    best_color(choose_rgb_for_theme(
        constants.orange_light,
        constants.orange_dark,
        default_bg(),
        default_fg(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_rgb_for_theme_prefers_detected_theme_and_defaults_to_dark() {
        for (background, foreground, expected) in [
            (Some((255, 255, 255)), None, (1, 2, 3)),
            (Some((0, 0, 0)), None, (4, 5, 6)),
            (None, Some((255, 255, 255)), (4, 5, 6)),
            (None, Some((0, 0, 0)), (1, 2, 3)),
            (None, None, (4, 5, 6)),
        ] {
            assert_eq!(
                choose_rgb_for_theme((1, 2, 3), (4, 5, 6), background, foreground),
                expected,
                "background={background:?}, foreground={foreground:?}"
            );
        }
    }

    #[test]
    fn parse_hex_rgb_accepts_rrggbb_and_rejects_invalid_inputs() {
        for (input, expected) in [
            ("#000000", Some((0, 0, 0))),
            ("#D68C27", Some((0xD6, 0x8C, 0x27))),
            ("F5A742", Some((0xF5, 0xA7, 0x42))),
            ("#nope", None),
            ("#12345", None),
        ] {
            assert_eq!(parse_hex_rgb(input), expected, "input: {input}");
        }
    }
}
