//! Terminal configuration loading.
//!
//! This intentionally starts as a small, typed subset of Ghostty-style
//! `key = value` config that maps onto `TerminalView`'s Phase 1 API.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Bar,
    Block,
    Underline,
    HollowBlock,
}

impl CursorStyle {
    pub fn terminal_view_value(self) -> i32 {
        match self {
            Self::Bar => 0,
            Self::Block => 1,
            Self::Underline => 2,
            Self::HollowBlock => 3,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TerminalConfig {
    pub font_family: Option<String>,
    pub font_point_size: Option<f64>,
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub cursor_color: Option<Rgb>,
    pub color_palette: Option<Vec<Rgb>>,
    pub cursor_style: Option<CursorStyle>,
    pub cursor_blink: Option<bool>,
    pub scrollback_limit: Option<usize>,
}

impl TerminalConfig {
    pub fn load_standard() -> Self {
        let mut config = Self::default();
        for path in standard_paths() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => config.merge(Self::parse(&contents)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => log::warn!("could not read config {}: {err}", path.display()),
            }
        }
        config
    }

    pub fn parse(input: &str) -> Self {
        let mut config = Self::default();
        let mut palette = xterm_256_palette();
        let mut palette_changed = false;

        for raw_line in input.lines() {
            let Some((key, value)) = parse_line(raw_line) else {
                continue;
            };
            match key.as_str() {
                "font-family" => config.font_family = parse_optional_string(&value),
                "font-size" => config.font_point_size = parse_f64(&value),
                "foreground" => config.foreground = parse_rgb(&value),
                "background" => config.background = parse_rgb(&value),
                "cursor-color" => config.cursor_color = parse_rgb(&value),
                "cursor-style" => config.cursor_style = parse_cursor_style(&value),
                "cursor-style-blink" => config.cursor_blink = parse_bool(&value),
                "scrollback-limit" => config.scrollback_limit = parse_usize(&value),
                "palette" => {
                    if apply_palette_entry(&value, &mut palette) {
                        palette_changed = true;
                    }
                }
                key if key.starts_with("color") => {
                    if let Some(index) = key.strip_prefix("color").and_then(parse_palette_index)
                        && let Some(rgb) = parse_rgb(&value)
                    {
                        palette[index] = rgb;
                        palette_changed = true;
                    }
                }
                _ => {}
            }
        }

        if palette_changed {
            config.color_palette = Some(palette.into());
        }

        config
    }

    pub fn merge(&mut self, other: Self) {
        if other.font_family.is_some() {
            self.font_family = other.font_family;
        }
        if other.font_point_size.is_some() {
            self.font_point_size = other.font_point_size;
        }
        if other.foreground.is_some() {
            self.foreground = other.foreground;
        }
        if other.background.is_some() {
            self.background = other.background;
        }
        if other.cursor_color.is_some() {
            self.cursor_color = other.cursor_color;
        }
        if other.color_palette.is_some() {
            self.color_palette = other.color_palette;
        }
        if other.cursor_style.is_some() {
            self.cursor_style = other.cursor_style;
        }
        if other.cursor_blink.is_some() {
            self.cursor_blink = other.cursor_blink;
        }
        if other.scrollback_limit.is_some() {
            self.scrollback_limit = other.scrollback_limit;
        }
    }
}

pub fn standard_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(path) = std::env::var("TAKO_CONFIG") {
        paths.push(PathBuf::from(path));
        return paths;
    }

    if let Some(config_home) = config_home() {
        paths.push(config_home.join("ghostty").join("config"));
        paths.push(config_home.join("tako").join("config"));
    }
    paths
}

fn config_home() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("XDG_CONFIG_HOME")
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    std::env::var("HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .map(|home| Path::new(&home).join(".config"))
}

fn parse_line(raw_line: &str) -> Option<(String, String)> {
    let line = raw_line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let (key, value) = line.split_once('=').or_else(|| line.split_once(':'))?;
    let key = key.trim().to_ascii_lowercase();
    if key.is_empty() {
        return None;
    }
    Some((key, clean_value(value)))
}

fn clean_value(value: &str) -> String {
    let trimmed = value.trim();
    let without_comment = trimmed
        .split_once(" #")
        .map_or(trimmed, |(before, _)| before.trim_end());
    without_comment
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            without_comment
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(without_comment)
        .trim()
        .to_string()
}

fn parse_optional_string(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_f64(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

fn parse_usize(value: &str) -> Option<usize> {
    value
        .replace('_', "")
        .parse::<usize>()
        .ok()
        .filter(|v| *v > 0)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_cursor_style(value: &str) -> Option<CursorStyle> {
    match value.to_ascii_lowercase().replace('_', "-").as_str() {
        "bar" => Some(CursorStyle::Bar),
        "block" => Some(CursorStyle::Block),
        "underline" => Some(CursorStyle::Underline),
        "hollow-block" | "block-hollow" => Some(CursorStyle::HollowBlock),
        _ => None,
    }
}

fn parse_rgb(value: &str) -> Option<Rgb> {
    let hex = value
        .strip_prefix('#')
        .or_else(|| value.strip_prefix("0x"))
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);

    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
            Some(Rgb {
                r: r * 17,
                g: g * 17,
                b: b * 17,
            })
        }
        6 => Some(Rgb {
            r: u8::from_str_radix(&hex[0..2], 16).ok()?,
            g: u8::from_str_radix(&hex[2..4], 16).ok()?,
            b: u8::from_str_radix(&hex[4..6], 16).ok()?,
        }),
        _ => None,
    }
}

fn apply_palette_entry(value: &str, palette: &mut [Rgb; 256]) -> bool {
    let Some((index, color)) = value.split_once('=') else {
        return false;
    };
    let Some(index) = parse_palette_index(index.trim()) else {
        return false;
    };
    let Some(rgb) = parse_rgb(color.trim()) else {
        return false;
    };
    palette[index] = rgb;
    true
}

fn parse_palette_index(value: &str) -> Option<usize> {
    let value = value.trim();
    let parsed = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        usize::from_str_radix(hex, 16).ok()?
    } else if let Some(bin) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        usize::from_str_radix(bin, 2).ok()?
    } else if let Some(oct) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        usize::from_str_radix(oct, 8).ok()?
    } else {
        value.parse::<usize>().ok()?
    };
    (parsed < 256).then_some(parsed)
}

fn xterm_256_palette() -> [Rgb; 256] {
    let mut palette = [Rgb { r: 0, g: 0, b: 0 }; 256];
    let base = [
        0x00_00_00, 0xcd_00_00, 0x00_cd_00, 0xcd_cd_00, 0x00_00_ee, 0xcd_00_cd, 0x00_cd_cd,
        0xe5_e5_e5, 0x7f_7f_7f, 0xff_00_00, 0x00_ff_00, 0xff_ff_00, 0x5c_5c_ff, 0xff_00_ff,
        0x00_ff_ff, 0xff_ff_ff,
    ];
    for (idx, value) in base.into_iter().enumerate() {
        palette[idx] = rgb_from_u32(value);
    }

    let levels = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    let mut idx = 16;
    for r in levels {
        for g in levels {
            for b in levels {
                palette[idx] = Rgb { r, g, b };
                idx += 1;
            }
        }
    }
    for i in 0..24 {
        let value = (8 + i * 10) as u8;
        palette[232 + i] = Rgb {
            r: value,
            g: value,
            b: value,
        };
    }
    palette
}

fn rgb_from_u32(value: u32) -> Rgb {
    Rgb {
        r: ((value >> 16) & 0xff) as u8,
        g: ((value >> 8) & 0xff) as u8,
        b: (value & 0xff) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ghostty_terminal_defaults() {
        let config = TerminalConfig::parse(
            r##"
            font-family = "Berkeley Mono"
            font-size = 14.5
            foreground = #d0d0d0
            background = 111111
            cursor-color = #f80
            cursor-style = underline
            cursor-style-blink = true
            scrollback-limit = 50000
            "##,
        );

        assert_eq!(config.font_family.as_deref(), Some("Berkeley Mono"));
        assert_eq!(config.font_point_size, Some(14.5));
        assert_eq!(
            config.foreground,
            Some(Rgb {
                r: 0xd0,
                g: 0xd0,
                b: 0xd0
            })
        );
        assert_eq!(
            config.background,
            Some(Rgb {
                r: 0x11,
                g: 0x11,
                b: 0x11
            })
        );
        assert_eq!(
            config.cursor_color,
            Some(Rgb {
                r: 0xff,
                g: 0x88,
                b: 0x00
            })
        );
        assert_eq!(config.cursor_style, Some(CursorStyle::Underline));
        assert_eq!(config.cursor_blink, Some(true));
        assert_eq!(config.scrollback_limit, Some(50000));
    }

    #[test]
    fn parses_palette_entries_and_color_aliases() {
        let config = TerminalConfig::parse(
            r##"
            palette = 5=#bb78d9
            palette = 0x10=0x123456
            color1 = #ff0000
            "##,
        );

        let palette = config.color_palette.expect("palette should be synthesized");
        assert_eq!(palette.len(), 256);
        assert_eq!(
            palette[1],
            Rgb {
                r: 0xff,
                g: 0,
                b: 0
            }
        );
        assert_eq!(
            palette[5],
            Rgb {
                r: 0xbb,
                g: 0x78,
                b: 0xd9
            }
        );
        assert_eq!(
            palette[16],
            Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        );
    }

    #[test]
    fn later_configs_override_earlier_values() {
        let mut config = TerminalConfig::parse("font-size = 12\nbackground = #000000");
        config.merge(TerminalConfig::parse(
            "font-size = 15\nforeground = #ffffff",
        ));

        assert_eq!(config.font_point_size, Some(15.0));
        assert_eq!(config.background, Some(Rgb { r: 0, g: 0, b: 0 }));
        assert_eq!(
            config.foreground,
            Some(Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff
            })
        );
    }
}
