//! Temas de color del panel: sustituyen el color por defecto (primer plano y fondo) y los
//! 16 colores ANSI. El tema `default` no toca nada y deja la paleta del terminal anfitrión.

use friki_term::Color as TColor;
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    fg: Color,
    bg: Color,
    /// `None` = colores indexados tal cual (paleta del terminal).
    ansi: Option<[Color; 16]>,
}

const fn rgb(h: u32) -> Color {
    Color::Rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            fg: Color::Reset,
            bg: Color::Reset,
            ansi: None,
        }
    }
}

const SOLARIZED_DARK: Theme = Theme {
    fg: rgb(0x0083_9496),
    bg: rgb(0x0000_2b36),
    ansi: Some([
        rgb(0x0007_3642),
        rgb(0x00dc_322f),
        rgb(0x0085_9900),
        rgb(0x00b5_8900),
        rgb(0x0026_8bd2),
        rgb(0x00d3_3682),
        rgb(0x002a_a198),
        rgb(0x00ee_e8d5),
        rgb(0x0000_2b36),
        rgb(0x00cb_4b16),
        rgb(0x0058_6e75),
        rgb(0x0065_7b83),
        rgb(0x0083_9496),
        rgb(0x006c_71c4),
        rgb(0x0093_a1a1),
        rgb(0x00fd_f6e3),
    ]),
};

const LIGHT: Theme = Theme {
    fg: rgb(0x001f_1f1f),
    bg: rgb(0x00fa_fafa),
    ansi: Some([
        rgb(0x0000_0000),
        rgb(0x00c9_1b00),
        rgb(0x0000_a600),
        rgb(0x0099_8500),
        rgb(0x0002_25c7),
        rgb(0x00c9_30c7),
        rgb(0x0000_8f9a),
        rgb(0x00b0_b0b0),
        rgb(0x0068_6868),
        rgb(0x00e5_0000),
        rgb(0x0000_b300),
        rgb(0x008a_7a00),
        rgb(0x001f_3fe0),
        rgb(0x00d9_40d6),
        rgb(0x0000_a0ad),
        rgb(0x005a_5a5a),
    ]),
};

impl Theme {
    /// Tema por nombre (sin distinguir mayúsculas). Uno desconocido da el `default` y el
    /// aviso correspondiente.
    pub fn named(name: &str) -> (Self, Option<String>) {
        match name.trim().to_ascii_lowercase().as_str() {
            "default" | "" => (Self::default(), None),
            "solarized-dark" => (SOLARIZED_DARK, None),
            "light" => (LIGHT, None),
            _ => (
                Self::default(),
                Some(format!(
                    "tema `{name}` desconocido (default, solarized-dark, light): se usa default"
                )),
            ),
        }
    }

    /// Color de ratatui de una celda; `bg` distingue el fondo del primer plano.
    pub fn color(&self, c: TColor, bg: bool) -> Color {
        match c {
            TColor::Default => {
                if bg {
                    self.bg
                } else {
                    self.fg
                }
            }
            TColor::Indexed(i) => self
                .ansi
                .and_then(|p| p.get(usize::from(i)).copied())
                .unwrap_or(Color::Indexed(i)),
            TColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_is_transparent() {
        let (t, w) = Theme::named("default");
        assert!(w.is_none());
        assert_eq!(t.color(TColor::Default, false), Color::Reset);
        assert_eq!(t.color(TColor::Default, true), Color::Reset);
        assert_eq!(t.color(TColor::Indexed(1), false), Color::Indexed(1));
    }

    #[test]
    fn themes_remap_default_and_ansi_only() {
        let (t, _) = Theme::named("Solarized-Dark");
        assert_eq!(t.color(TColor::Default, true), Color::Rgb(0, 0x2b, 0x36));
        assert_eq!(
            t.color(TColor::Indexed(1), false),
            Color::Rgb(0xdc, 0x32, 0x2f)
        );
        assert_eq!(t.color(TColor::Indexed(200), false), Color::Indexed(200));
        assert_eq!(t.color(TColor::Rgb(1, 2, 3), true), Color::Rgb(1, 2, 3));
        let (l, _) = Theme::named("light");
        assert_eq!(l.color(TColor::Default, true), Color::Rgb(0xfa, 0xfa, 0xfa));
        assert_ne!(l, t);
    }

    #[test]
    fn unknown_theme_warns_and_falls_back() {
        let (t, w) = Theme::named("neon");
        assert_eq!(t, Theme::default());
        assert!(w.is_some_and(|w| w.contains("neon")));
    }
}
