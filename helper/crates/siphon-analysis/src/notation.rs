//! Canonical ASCII key names, the two DJ circle-of-fifths notations, and the
//! filename suffix template.

use serde::Serialize;

const MAJOR: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
const MINOR: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];
const CAMELOT_MAJOR: [u32; 12] = [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1];
const CAMELOT_MINOR: [u32; 12] = [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10];

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyNames {
    pub key: String,
    pub key_short: String,
    pub tonic: String,
    pub mode: String,
    pub camelot: String,
    pub open_key: String,
}

/// All Analysis key fields for pitch class 0=C, ..., 11=B.
pub fn key_names(tonic: usize, minor: bool) -> KeyNames {
    let name = if minor { MINOR[tonic] } else { MAJOR[tonic] };
    let number = if minor { CAMELOT_MINOR[tonic] } else { CAMELOT_MAJOR[tonic] };
    let mode = if minor { "minor" } else { "major" };
    KeyNames {
        key: format!("{name} {mode}"),
        key_short: format!("{name}{}", if minor { "m" } else { "" }),
        tonic: name.to_string(),
        mode: mode.to_string(),
        camelot: format!("{number}{}", if minor { 'A' } else { 'B' }),
        open_key: format!("{}{}", (number + 4) % 12 + 1, if minor { 'm' } else { 'd' }),
    }
}

/// Parse names (including enharmonics/Unicode), Camelot, or Open Key. Returns (tonic, minor).
pub fn parse_key(value: &str) -> Result<(usize, bool), String> {
    let value = value.trim().replace('♯', "#").replace('♭', "b");
    let invalid = || format!("Invalid musical key: '{value}'");
    // Camelot (8A/8B) or Open Key (1m/1d).
    if let Some(suffix) = value.chars().last().filter(|c| "ABmd".contains(*c)) {
        let digits = &value[..value.len() - 1];
        if !digits.is_empty() && !digits.starts_with('0') && digits.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(number @ 1..=12) = digits.parse::<u32>() {
                let minor = matches!(suffix, 'A' | 'm');
                let number = if matches!(suffix, 'm' | 'd') { (number + 6) % 12 + 1 } else { number };
                let table = if minor { &CAMELOT_MINOR } else { &CAMELOT_MAJOR };
                return Ok((table.iter().position(|n| *n == number).unwrap(), minor));
            }
        }
    }
    let mut chars = value.chars();
    let letter = chars.next().filter(|c| "ABCDEFGabcdefg".contains(*c)).ok_or_else(invalid)?;
    let mut rest = chars.as_str();
    let accidental = if let Some(r) = rest.strip_prefix('#') {
        rest = r;
        1
    } else if let Some(r) = rest.strip_prefix('b') {
        rest = r;
        -1
    } else {
        0
    };
    let minor = match rest.trim_start() {
        "" if rest.is_empty() => false,
        "major" | "maj" | "M" => false,
        "minor" | "min" | "m" => true,
        _ => return Err(invalid()),
    };
    let pitch: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        _ => 11,
    };
    Ok(((pitch + accidental).rem_euclid(12) as usize, minor))
}

pub fn parse_key_names(value: &str) -> Result<KeyNames, String> {
    let (tonic, minor) = parse_key(value)?;
    Ok(key_names(tonic, minor))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Notation {
    #[default]
    Standard,
    Camelot,
    Openkey,
}

/// Format only supported placeholders, then make a Windows-safe suffix of at most 160 characters.
///
/// The leading space of the default template is intentional; the caller handles path
/// length and collisions. Unknown placeholders or format specs are rejected.
pub fn format_suffix(bpm: f64, key: &str, template: &str, notation: Notation) -> Result<String, String> {
    if !bpm.is_finite() || bpm <= 0.0 {
        return Err("bpm must be finite and positive".into());
    }
    let names = parse_key_names(key)?;
    let rounded = bpm.round_ties_even();
    // {bpm} is a whole number (filenames read better); {bpmExact} keeps one decimal.
    let exact = if (bpm - rounded).abs() < 0.05 { format!("{}", rounded as i64) } else { format!("{bpm:.1}") };
    let field = |name: &str| -> Option<String> {
        Some(match name {
            "bpm" => format!("{}", (bpm + 0.5).floor() as i64),
            "bpmExact" => exact.clone(),
            "keyShort" => names.key_short.clone(),
            "keyLong" => names.key.clone(),
            "camelot" => names.camelot.clone(),
            "openkey" => names.open_key.clone(),
            "key" => match notation {
                Notation::Standard => names.key_short.clone(),
                Notation::Camelot => names.camelot.clone(),
                Notation::Openkey => names.open_key.clone(),
            },
            _ => return None,
        })
    };
    let mut result = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                result.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                result.push('}');
            }
            '}' => return Err("Single '}' encountered in format string".into()),
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => name.push(c),
                        None => return Err("expected '}' before end of string".into()),
                    }
                }
                match field(&name) {
                    Some(value) => result.push_str(&value),
                    None => return Err(format!("Unsupported suffix placeholder: '{name}'")),
                }
            }
            _ => result.push(c),
        }
    }
    let safe: String = result
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 0x20 || c == '\x7f' { '_' } else { c })
        .take(160)
        .collect();
    Ok(safe.trim_end_matches([' ', '.']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_dj_notations() {
        let am = key_names(9, true);
        assert_eq!(
            (am.key.as_str(), am.key_short.as_str(), am.camelot.as_str(), am.open_key.as_str()),
            ("A minor", "Am", "8A", "1m")
        );
        let c = key_names(0, false);
        assert_eq!((c.camelot.as_str(), c.open_key.as_str()), ("8B", "1d"));
        assert_eq!(key_names(6, false).key, "Gb major");
        assert_eq!(key_names(1, true).key_short, "C#m");
    }

    #[test]
    fn parses_names_enharmonics_and_dj_codes() {
        for (text, expected) in [
            ("Am", (9, true)),
            ("A minor", (9, true)),
            ("a", (9, false)),
            ("F♯m", (6, true)),
            ("Bb", (10, false)),
            ("bb", (10, false)),
            ("Ebm", (3, true)),
            ("Eb major", (3, false)),
            ("B", (11, false)),
            ("Cb", (11, false)),
            ("8A", (9, true)),
            ("8B", (0, false)),
            ("1m", (9, true)),
            ("1d", (0, false)),
            ("12B", (4, false)),
            ("C M", (0, false)),
            ("Dbmaj", (1, false)),
        ] {
            assert_eq!(parse_key(text).unwrap(), expected, "{text}");
        }
        for bad in ["H", "13A", "0A", "A minr", "", "Bb b", "08A", "Cm7"] {
            assert!(parse_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn suffix_placeholders_escapes_and_sanitizing() {
        assert_eq!(format_suffix(120.5, "Am", " [{bpm} BPM {key}]", Notation::Standard).unwrap(), " [121 BPM Am]");
        assert_eq!(format_suffix(127.96, "Am", "{bpmExact}", Notation::Standard).unwrap(), "128");
        assert_eq!(format_suffix(127.9, "Am", "{bpmExact}", Notation::Standard).unwrap(), "127.9");
        assert_eq!(format_suffix(120.0, "Am", " {key} {{x}}", Notation::Camelot).unwrap(), " 8A {x}");
        assert_eq!(format_suffix(120.0, "Am", " {keyLong}/{openkey}: ", Notation::Openkey).unwrap(), " A minor_1m_");
        assert!(format_suffix(120.0, "Am", "{bpm:3}", Notation::Standard).is_err());
        assert!(format_suffix(120.0, "Am", "{nope}", Notation::Standard).is_err());
        assert!(format_suffix(120.0, "Am", "{", Notation::Standard).is_err());
        assert!(format_suffix(120.0, "Am", "}", Notation::Standard).is_err());
        assert_eq!(format_suffix(120.0, "Am", &"x".repeat(300), Notation::Standard).unwrap().len(), 160);
    }
}
