mod chr;
mod palette;
#[cfg(test)]
mod tests;

pub use chr::png_to_chr;
pub use palette::{nearest_nes_color, NES_COLORS};
