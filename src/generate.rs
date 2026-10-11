//! Secret generators. Everything is drawn from the operating system's CSPRNG.

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*()-_=+[]{};:,.<>?/~";
/// Characters that are easy to misread when a password has to be copied by eye.
const LOOK_ALIKES: &str = "Il1|O0o";

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PasswordOptions {
    pub length: usize,
    pub lower: bool,
    pub upper: bool,
    pub digits: bool,
    pub symbols: bool,
    pub avoid_look_alikes: bool,
}

impl Default for PasswordOptions {
    fn default() -> Self {
        Self {
            length: 20,
            lower: true,
            upper: true,
            digits: true,
            symbols: true,
            avoid_look_alikes: true,
        }
    }
}

impl PasswordOptions {
    pub const MIN_LENGTH: usize = 4;
    pub const MAX_LENGTH: usize = 128;

    fn classes(&self) -> Vec<Vec<char>> {
        [
            (self.lower, LOWER),
            (self.upper, UPPER),
            (self.digits, DIGITS),
            (self.symbols, SYMBOLS),
        ]
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .map(|(_, set)| {
            set.chars()
                .filter(|c| !(self.avoid_look_alikes && LOOK_ALIKES.contains(*c)))
                .collect()
        })
        .collect()
    }

    /// Approximate strength in bits (ignores the small loss from requiring every class).
    pub fn entropy_bits(&self) -> f64 {
        let alphabet: usize = self.classes().iter().map(Vec::len).sum();
        if alphabet == 0 {
            return 0.0;
        }
        self.length as f64 * (alphabet as f64).log2()
    }
}

fn fill(buf: &mut [u8]) {
    getrandom::fill(buf).expect("the operating system's random source is unavailable");
}

/// Uniform integer in `0..n`, using rejection sampling so there is no modulo bias.
fn below(n: u32) -> u32 {
    assert!(n > 0);
    // Largest value such that `zone + 1` is a multiple of `n`.
    let zone = u32::MAX - (u32::MAX % n + 1) % n;
    loop {
        let mut bytes = [0u8; 4];
        fill(&mut bytes);
        let v = u32::from_le_bytes(bytes);
        if v <= zone {
            return v % n;
        }
    }
}

/// A password containing at least one character from every enabled class.
/// Returns `None` when no class is enabled.
pub fn password(opts: &PasswordOptions) -> Option<String> {
    let classes = opts.classes();
    if classes.is_empty() {
        return None;
    }
    let alphabet: Vec<char> = classes.iter().flatten().copied().collect();
    let length = opts
        .length
        .clamp(PasswordOptions::MIN_LENGTH, PasswordOptions::MAX_LENGTH);
    // Redrawing the whole password (rather than patching characters in) keeps the
    // result uniform over all passwords that satisfy the class requirement.
    loop {
        let candidate: Vec<char> = (0..length)
            .map(|_| alphabet[below(alphabet.len() as u32) as usize])
            .collect();
        if classes
            .iter()
            .all(|class| candidate.iter().any(|c| class.contains(c)))
        {
            return Some(candidate.into_iter().collect());
        }
    }
}

/// A random (version 4) GUID/UUID: 122 random bits.
pub fn guid(uppercase: bool) -> String {
    let mut b = [0u8; 16];
    fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let h = hex(&b);
    let s = format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    );
    if uppercase { s.to_uppercase() } else { s }
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum KeyFormat {
    Hex,
    Base64,
}

/// `bytes` random bytes, e.g. for an API secret or encryption key.
pub fn random_key(bytes: usize, format: KeyFormat) -> String {
    let mut buf = vec![0u8; bytes];
    fill(&mut buf);
    match format {
        KeyFormat::Hex => hex(&buf),
        KeyFormat::Base64 => base64(&buf),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_has_requested_length_and_every_class() {
        for length in [4, 5, 20, 128] {
            let opts = PasswordOptions {
                length,
                avoid_look_alikes: false,
                ..Default::default()
            };
            for _ in 0..200 {
                let pw = password(&opts).unwrap();
                assert_eq!(pw.chars().count(), length);
                for set in [LOWER, UPPER, DIGITS, SYMBOLS] {
                    assert!(pw.chars().any(|c| set.contains(c)), "{pw} lacks a class");
                }
            }
        }
    }

    #[test]
    fn password_respects_disabled_classes_and_look_alikes() {
        let opts = PasswordOptions {
            length: 64,
            symbols: false,
            upper: false,
            avoid_look_alikes: true,
            ..Default::default()
        };
        for _ in 0..200 {
            let pw = password(&opts).unwrap();
            assert!(pw.chars().all(|c| LOWER.contains(c) || DIGITS.contains(c)));
            assert!(!pw.chars().any(|c| LOOK_ALIKES.contains(c)));
        }
    }

    #[test]
    fn password_needs_at_least_one_class() {
        let opts = PasswordOptions {
            lower: false,
            upper: false,
            digits: false,
            symbols: false,
            ..Default::default()
        };
        assert_eq!(password(&opts), None);
        assert_eq!(opts.entropy_bits(), 0.0);
    }

    #[test]
    fn below_stays_in_range_and_reaches_every_value() {
        let mut seen = [false; 7];
        for _ in 0..2000 {
            seen[below(7) as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
        assert_eq!(below(1), 0);
    }

    #[test]
    fn guid_is_version_4() {
        for _ in 0..200 {
            let g = guid(false);
            let parts: Vec<&str> = g.split('-').collect();
            assert_eq!(
                parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
                [8, 4, 4, 4, 12]
            );
            assert!(g.chars().all(|c| c == '-' || "0123456789abcdef".contains(c)));
            assert!(parts[2].starts_with('4'));
            assert!("89ab".contains(&parts[3][0..1]));
        }
        assert_ne!(guid(false), guid(false));
        let upper = guid(true);
        assert_eq!(upper, upper.to_uppercase());
    }

    #[test]
    fn encodings_match_known_vectors() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
        assert_eq!(random_key(32, KeyFormat::Hex).len(), 64);
        assert_eq!(random_key(32, KeyFormat::Base64).len(), 44);
    }
}
