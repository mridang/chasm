use crate::random::Random;
use fake::faker::color::en::{HslColor, HslaColor, RgbColor, RgbaColor};
use fake::Fake;

/// Generates an RGB color string like "rgb(120,200,40)".
pub fn generate_rgb_color(rng: &mut Random) -> String {
    RgbColor().fake_with_rng(rng.inner())
}

/// Generates an RGBA color string with an alpha component.
pub fn generate_rgba_color(rng: &mut Random) -> String {
    RgbaColor().fake_with_rng(rng.inner())
}

/// Generates an HSL color string like "hsl(210,50%,40%)".
pub fn generate_hsl_color(rng: &mut Random) -> String {
    HslColor().fake_with_rng(rng.inner())
}

/// Generates an HSLA color string with an alpha component.
pub fn generate_hsla_color(rng: &mut Random) -> String {
    HslaColor().fake_with_rng(rng.inner())
}

/// Generates a single-line RGB color string like `"rgb(120,200,40)"`.
///
/// The upstream `fake::faker::color::en::Color` distribution joins all color
/// representations with `\n`, which is invalid for `format: color`. This
/// generator picks the RGB form so the result is always a single line.
pub fn generate_color(rng: &mut Random) -> String {
    RgbColor().fake_with_rng(rng.inner())
}
