//! 托盘图标的像素。
//!
//! Slint 的托盘只接受一张图，没有单独的数字徽标。逾期数量画在这张图上。
//! 数量为 0 时不画数字。图标底色不是产品规格，只是要在浅色和深色托盘上都看得见。

/// 托盘图标的边长，单位是像素。
pub const TRAY_ICON_PX: u32 = 32;

const BACKGROUND: [u8; 4] = [0x20, 0x46, 0x82, 0xFF];
const DIGIT: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

/// 5×7 的数字。`#` 是笔画。
const GLYPHS: [&str; 10] = [
    "\
.###.\
#...#\
#..##\
#.#.#\
##..#\
#...#\
.###.",
    "\
..#..\
.##..\
..#..\
..#..\
..#..\
..#..\
.###.",
    "\
.###.\
#...#\
....#\
..##.\
.#...\
#....\
#####",
    "\
.###.\
#...#\
....#\
..##.\
....#\
#...#\
.###.",
    "\
...#.\
..##.\
.#.#.\
#..#.\
#####\
...#.\
...#.",
    "\
#####\
#....\
####.\
....#\
....#\
#...#\
.###.",
    "\
.###.\
#....\
#....\
####.\
#...#\
#...#\
.###.",
    "\
#####\
....#\
...#.\
..#..\
.#...\
.#...\
.#...",
    "\
.###.\
#...#\
#...#\
.###.\
#...#\
#...#\
.###.",
    "\
.###.\
#...#\
#...#\
.####\
....#\
...#.\
.##..",
];

/// RGBA，长度是 [`TRAY_ICON_PX`] 的平方乘 4。`overdue` 为 0 时没有白色数字像素。
#[must_use]
pub fn tray_icon_rgba(overdue: u32) -> Vec<u8> {
    let width = TRAY_ICON_PX as usize;
    let mut pixels = vec![0u8; width * width * 4];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&BACKGROUND);
    }
    if overdue == 0 {
        return pixels;
    }
    let text = overdue.to_string();
    let glyph_w = 5usize;
    let glyph_h = 7usize;
    let gap = 1usize;
    let text_w = text.len() * glyph_w + text.len().saturating_sub(1) * gap;
    let origin_x = width.saturating_sub(text_w) / 2;
    let origin_y = width.saturating_sub(glyph_h) / 2;
    for (index, ch) in text.chars().enumerate() {
        let Some(digit) = ch.to_digit(10) else {
            continue;
        };
        let glyph = GLYPHS[digit as usize].as_bytes();
        let x0 = origin_x + index * (glyph_w + gap);
        for row in 0..glyph_h {
            for col in 0..glyph_w {
                if glyph.get(row * glyph_w + col) != Some(&b'#') {
                    continue;
                }
                let x = x0 + col;
                let y = origin_y + row;
                if x >= width || y >= width {
                    continue;
                }
                let offset = (y * width + x) * 4;
                pixels[offset..offset + 4].copy_from_slice(&DIGIT);
            }
        }
    }
    pixels
}

/// 是否画了数字。数量为 0 时为假。
#[must_use]
pub fn has_badge_pixels(rgba: &[u8]) -> bool {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[0] == DIGIT[0] && pixel[1] == DIGIT[1] && pixel[2] == DIGIT[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_hides_the_digits_and_a_count_draws_them() {
        let plain = tray_icon_rgba(0);
        let one = tray_icon_rgba(1);
        let twelve = tray_icon_rgba(12);
        assert_eq!(plain.len(), (TRAY_ICON_PX * TRAY_ICON_PX * 4) as usize);
        assert!(!has_badge_pixels(&plain));
        assert!(has_badge_pixels(&one));
        assert!(has_badge_pixels(&twelve));
        assert_ne!(one, twelve);
    }
}
