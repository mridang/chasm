use crate::random::Random;

/// Generates a random UUID v4 string in standard hyphenated format.
pub fn generate(rng: &mut Random) -> String {
    let hex: &[u8] = b"0123456789abcdef";
    let mut bytes = [0u8; 16];
    for b in bytes.iter_mut() {
        *b = rng.byte();
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut chars = Vec::with_capacity(32);
    for &b in bytes.iter() {
        chars.push(hex[(b >> 4) as usize] as char);
        chars.push(hex[(b & 0x0f) as usize] as char);
    }
    format!(
        "{}-{}-{}-{}-{}",
        chars[0..8].iter().collect::<String>(),
        chars[8..12].iter().collect::<String>(),
        chars[12..16].iter().collect::<String>(),
        chars[16..20].iter().collect::<String>(),
        chars[20..32].iter().collect::<String>()
    )
}
