//! EBCDIC CP037 (subconjunto: ASCII imprimible y letras latinas habituales).
//! Tabla propia: no hay crate de códigos de página mantenido y mínimo para esto.

/// EBCDIC -> carácter. Controles y bytes sin asignar se muestran como espacio.
pub fn to_char(b: u8) -> char {
    match b {
        0x81..=0x89 => (b'a' + (b - 0x81)) as char,
        0x91..=0x99 => (b'j' + (b - 0x91)) as char,
        0xA2..=0xA9 => (b's' + (b - 0xA2)) as char,
        0xC1..=0xC9 => (b'A' + (b - 0xC1)) as char,
        0xD1..=0xD9 => (b'J' + (b - 0xD1)) as char,
        0xE2..=0xE9 => (b'S' + (b - 0xE2)) as char,
        0xF0..=0xF9 => (b'0' + (b - 0xF0)) as char,
        0x42 => 'â',
        0x43 => 'ä',
        0x44 => 'à',
        0x45 => 'á',
        0x46 => 'ã',
        0x47 => 'å',
        0x48 => 'ç',
        0x49 => 'ñ',
        0x4A => '¢',
        0x4B => '.',
        0x4C => '<',
        0x4D => '(',
        0x4E => '+',
        0x4F => '|',
        0x50 => '&',
        0x51 => 'é',
        0x52 => 'ê',
        0x53 => 'ë',
        0x54 => 'è',
        0x55 => 'í',
        0x56 => 'î',
        0x57 => 'ï',
        0x58 => 'ì',
        0x59 => 'ß',
        0x5A => '!',
        0x5B => '$',
        0x5C => '*',
        0x5D => ')',
        0x5E => ';',
        0x5F => '¬',
        0x60 => '-',
        0x61 => '/',
        0x62 => 'Â',
        0x63 => 'Ä',
        0x64 => 'À',
        0x65 => 'Á',
        0x66 => 'Ã',
        0x67 => 'Å',
        0x68 => 'Ç',
        0x69 => 'Ñ',
        0x6A => '¦',
        0x6B => ',',
        0x6C => '%',
        0x6D => '_',
        0x6E => '>',
        0x6F => '?',
        0x70 => 'ø',
        0x71 => 'É',
        0x72 => 'Ê',
        0x73 => 'Ë',
        0x74 => 'È',
        0x75 => 'Í',
        0x76 => 'Î',
        0x77 => 'Ï',
        0x78 => 'Ì',
        0x79 => '`',
        0x7A => ':',
        0x7B => '#',
        0x7C => '@',
        0x7D => '\'',
        0x7E => '=',
        0x7F => '"',
        0xA0 => 'µ',
        0xA1 => '~',
        0xAA => '¡',
        0xAB => '¿',
        0xB0 => '^',
        0xBA => '[',
        0xBB => ']',
        0xC0 => '{',
        0xD0 => '}',
        0xE0 => '\\',
        _ => ' ',
    }
}

/// Carácter -> EBCDIC; `None` si no está en la tabla.
pub fn from_char(c: char) -> Option<u8> {
    if c == ' ' {
        return Some(0x40);
    }
    (0x42..=0xFFu8).find(|&b| to_char(b) == c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_y_vuelta() {
        for c in "Hola, Mundo 0123456789 ñÑ¿¡[]{}\\~^|!$%&*()-_=+;:'\",.<>/?@#`".chars() {
            let b = from_char(c).unwrap_or_else(|| panic!("sin código para {c}"));
            assert_eq!(to_char(b), c);
        }
        assert_eq!(from_char('\u{1F600}'), None);
        assert_eq!(to_char(0xC1), 'A');
        assert_eq!(to_char(0x00), ' ');
    }
}
