//! 托盘图标的像素。
//!
//! Slint 的托盘只接受一张图，没有单独的数字徽标。底图是应用图标。
//! 逾期数量不为 0 时，在右下角盖一块小徽章再画数字。数量为 0 时不画徽章。
//! 徽章的底色、边和放大倍数不是产品规格。

/// 托盘图标的边长，单位是像素。
pub const TRAY_ICON_PX: u32 = 32;

/// 32×32 的应用图标，自上而下 RGBA。由 `tools/render_app_icon.py` 从 256px 渲染缩小。
const BASE: &[u8] = include_bytes!("../../../app/assets/lanwork-32.rgba");

const _: () = assert!(BASE.len() == (TRAY_ICON_PX * TRAY_ICON_PX * 4) as usize);

/// 徽章白底。源图里没有这个纯白，数量为 0 时整张图也不该出现。
const FILL: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
/// 数字和徽章边。取自图标底色，白底上够深，看得清。
const INK: [u8; 4] = [0x1E, 0x1B, 0x4B, 0xFF];
/// 有更新时右上角的小点。源图里没有这个颜色，和右下角的逾期徽章不叠在一起。
const UPDATE_DOT: [u8; 4] = [0xFF, 0x5A, 0x1F, 0xFF];
const DOT: isize = 6;
const DOT_X: isize = 25;
const DOT_Y: isize = 1;

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const BORDER: usize = 1;
const PAD: usize = 1;

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

struct BadgeLayout {
    x: isize,
    y: isize,
    width: usize,
    height: usize,
    scale: usize,
    text_x: isize,
    text_y: isize,
}

fn badge_size(digits: usize, scale: usize) -> (usize, usize) {
    let text_w = digits * GLYPH_W * scale + digits.saturating_sub(1) * scale;
    let text_h = GLYPH_H * scale;
    (text_w + (BORDER + PAD) * 2, text_h + (BORDER + PAD) * 2)
}

/// 右下角对齐。一位和两位用 2 倍笔画，更长就用 1 倍；仍然宽过图标时向左伸出，画的时候裁掉。
fn badge_layout(digits: usize) -> BadgeLayout {
    let scale = if badge_size(digits, 2).0 <= TRAY_ICON_PX as usize
        && badge_size(digits, 2).1 <= TRAY_ICON_PX as usize
    {
        2
    } else {
        1
    };
    let (width, height) = badge_size(digits, scale);
    let side = isize::try_from(TRAY_ICON_PX).expect("icon side fits isize");
    let x = side - isize::try_from(width).expect("badge width fits isize");
    let y = side - isize::try_from(height).expect("badge height fits isize");
    let inset = isize::try_from(BORDER + PAD).expect("badge inset fits isize");
    BadgeLayout {
        x,
        y,
        width,
        height,
        scale,
        text_x: x + inset,
        text_y: y + inset,
    }
}

fn put(pixels: &mut [u8], x: isize, y: isize, color: [u8; 4]) {
    if x < 0 || y < 0 {
        return;
    }
    let width = TRAY_ICON_PX as usize;
    let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
        return;
    };
    if x >= width || y >= width {
        return;
    }
    let offset = (y * width + x) * 4;
    pixels[offset..offset + 4].copy_from_slice(&color);
}

fn paint_badge(pixels: &mut [u8], overdue: u32) {
    let text = overdue.to_string();
    let layout = badge_layout(text.len());
    let width = isize::try_from(layout.width).expect("badge width fits isize");
    let height = isize::try_from(layout.height).expect("badge height fits isize");
    let border = isize::try_from(BORDER).expect("border fits isize");
    for y in 0..height {
        for x in 0..width {
            let edge = x < border || y < border || x >= width - border || y >= height - border;
            put(
                pixels,
                layout.x + x,
                layout.y + y,
                if edge { INK } else { FILL },
            );
        }
    }
    let step = isize::try_from(GLYPH_W * layout.scale + layout.scale).expect("glyph step");
    let scale = isize::try_from(layout.scale).expect("scale fits isize");
    for (index, ch) in text.chars().enumerate() {
        let Some(digit) = ch.to_digit(10) else {
            continue;
        };
        let glyph = GLYPHS[digit as usize].as_bytes();
        let x0 = layout.text_x + isize::try_from(index).expect("digit index") * step;
        for row in 0..GLYPH_H {
            for col in 0..GLYPH_W {
                if glyph.get(row * GLYPH_W + col) != Some(&b'#') {
                    continue;
                }
                let cell_x = x0 + isize::try_from(col).expect("col") * scale;
                let cell_y = layout.text_y + isize::try_from(row).expect("row") * scale;
                for sy in 0..scale {
                    for sx in 0..scale {
                        put(pixels, cell_x + sx, cell_y + sy, INK);
                    }
                }
            }
        }
    }
}

/// RGBA，长度是 [`TRAY_ICON_PX`] 的平方乘 4。`overdue` 为 0 时没有徽章。
#[must_use]
pub fn tray_icon_rgba(overdue: u32) -> Vec<u8> {
    tray_icon_marked(overdue, false)
}

/// 和 [`tray_icon_rgba`] 相同。`update` 为真时在右上角加一个小点。
#[must_use]
pub fn tray_icon_marked(overdue: u32, update: bool) -> Vec<u8> {
    let mut pixels = BASE.to_vec();
    if overdue != 0 {
        paint_badge(&mut pixels, overdue);
    }
    if update {
        paint_update_dot(&mut pixels);
    }
    pixels
}

fn paint_update_dot(pixels: &mut [u8]) {
    for y in 0..DOT {
        for x in 0..DOT {
            put(pixels, DOT_X + x, DOT_Y + y, UPDATE_DOT);
        }
    }
}

/// 是否画了徽章。数量为 0 时为假。看的是徽章白底。
#[must_use]
pub fn has_badge_pixels(rgba: &[u8]) -> bool {
    rgba.as_chunks::<4>().0.contains(&FILL)
}

/// 是否画了更新小点。
#[must_use]
pub fn has_update_dot(rgba: &[u8]) -> bool {
    rgba.as_chunks::<4>().0.contains(&UPDATE_DOT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICO: &[u8] = include_bytes!("../../../app/assets/lanwork.ico");
    const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

    fn pixel(rgba: &[u8], x: usize, y: usize) -> [u8; 4] {
        let width = TRAY_ICON_PX as usize;
        let offset = (y * width + x) * 4;
        rgba[offset..offset + 4].try_into().unwrap()
    }

    fn in_badge(layout: &BadgeLayout, x: isize, y: isize) -> bool {
        x >= layout.x
            && y >= layout.y
            && x < layout.x + layout.width as isize
            && y < layout.y + layout.height as isize
    }

    #[test]
    fn zero_is_the_app_icon_without_a_badge() {
        let plain = tray_icon_rgba(0);
        assert_eq!(plain, BASE);
        assert!(!has_badge_pixels(&plain));
        assert_eq!(pixel(&plain, 0, 0)[3], 0, "圆角外是透明的");
        assert_eq!(pixel(&plain, 16, 16)[3], 255, "图标中间不透明");
        let first: [u8; 4] = plain[..4].try_into().unwrap();
        assert!(
            plain.as_chunks::<4>().0.iter().any(|px| *px != first),
            "不是纯色块"
        );
        assert!(
            !BASE.as_chunks::<4>().0.contains(&FILL),
            "底图没有徽章白，否则数量为 0 也会被当成画了数字"
        );
        assert!(
            !BASE.as_chunks::<4>().0.contains(&UPDATE_DOT),
            "底图没有更新小点的颜色"
        );
        assert!(!has_update_dot(&plain));
    }

    #[test]
    fn update_dot_sits_in_the_top_right_and_keeps_the_overdue_badge() {
        let plain = tray_icon_rgba(0);
        let marked = tray_icon_marked(0, true);
        let both = tray_icon_marked(1, true);
        assert!(has_update_dot(&marked));
        assert!(has_update_dot(&both));
        assert!(has_badge_pixels(&both));
        assert!(!has_badge_pixels(&marked));
        for y in 0..DOT as usize {
            for x in 0..DOT as usize {
                assert_eq!(
                    pixel(&marked, DOT_X as usize + x, DOT_Y as usize + y),
                    UPDATE_DOT
                );
            }
        }
        let side = TRAY_ICON_PX as usize;
        for y in 0..side {
            for x in 0..side {
                let in_dot = (x as isize) >= DOT_X
                    && (x as isize) < DOT_X + DOT
                    && (y as isize) >= DOT_Y
                    && (y as isize) < DOT_Y + DOT;
                if in_dot {
                    continue;
                }
                assert_eq!(pixel(&marked, x, y), pixel(&plain, x, y));
            }
        }
        let layout = badge_layout(1);
        assert!(layout.y > DOT_Y + DOT, "逾期徽章在小点下面");
    }

    #[test]
    fn badge_draws_the_digit_shape_in_the_bottom_right() {
        let plain = tray_icon_rgba(0);
        let one = tray_icon_rgba(1);
        let twelve = tray_icon_rgba(12);
        assert!(has_badge_pixels(&one));
        assert!(has_badge_pixels(&twelve));
        assert_ne!(one, twelve);
        assert_glyph(&one, 1, 0);
        assert_glyph(&twelve, 12, 0);
        assert_glyph(&twelve, 12, 1);
        let layout = badge_layout(1);
        assert_eq!(layout.scale, 2);
        assert_eq!(layout.x + layout.width as isize, TRAY_ICON_PX as isize);
        assert_eq!(layout.y + layout.height as isize, TRAY_ICON_PX as isize);
        assert!(layout.x >= (TRAY_ICON_PX as isize) / 2);
        let side = TRAY_ICON_PX as usize;
        for y in 0..side {
            for x in 0..side {
                if in_badge(&layout, x as isize, y as isize) {
                    continue;
                }
                assert_eq!(pixel(&one, x, y), pixel(&plain, x, y));
            }
        }
    }

    #[test]
    fn longer_counts_use_a_smaller_scale_and_clip_instead_of_a_plus() {
        assert_eq!(badge_layout(2).scale, 2);
        assert_eq!(badge_layout(3).scale, 1);
        let hundred = tray_icon_rgba(100);
        assert_glyph(&hundred, 100, 0);
        assert_glyph(&hundred, 100, 2);
        let wide = tray_icon_rgba(1_000_000_000);
        assert!(has_badge_pixels(&wide));
        assert_glyph(&wide, 1_000_000_000, 9);
        let layout = badge_layout(10);
        assert!(layout.x < 0, "十位数字的徽章比图标宽，左侧要被裁掉");
        assert_eq!(wide.len(), BASE.len());
    }

    fn assert_glyph(rgba: &[u8], overdue: u32, index: usize) {
        let text = overdue.to_string();
        let layout = badge_layout(text.len());
        let digit = text.chars().nth(index).unwrap().to_digit(10).unwrap() as usize;
        let glyph = GLYPHS[digit].as_bytes();
        let step = (GLYPH_W * layout.scale + layout.scale) as isize;
        let scale = layout.scale as isize;
        let x0 = layout.text_x + index as isize * step;
        for row in 0..GLYPH_H {
            for col in 0..GLYPH_W {
                let ink = glyph[row * GLYPH_W + col] == b'#';
                for sy in 0..scale {
                    for sx in 0..scale {
                        let x = x0 + col as isize * scale + sx;
                        let y = layout.text_y + row as isize * scale + sy;
                        assert!(
                            x >= 0
                                && y >= 0
                                && x < TRAY_ICON_PX as isize
                                && y < TRAY_ICON_PX as isize,
                            "digit {digit} at ({x},{y}) falls outside the icon"
                        );
                        let got = pixel(rgba, x as usize, y as usize);
                        let want = if ink { INK } else { FILL };
                        assert_eq!(got, want, "digit {digit} cell ({col},{row})");
                    }
                }
            }
        }
    }

    #[test]
    fn ico_contains_each_size_and_the_32px_image_is_the_tray_base() {
        let images = decode_ico(ICO);
        let sizes: Vec<u32> = images.iter().map(|(size, _)| *size).collect();
        assert_eq!(sizes, SIZES);
        let (_, pixels) = images.iter().find(|(size, _)| *size == 32).unwrap();
        assert_eq!(pixels, BASE);
    }

    fn decode_ico(bytes: &[u8]) -> Vec<(u32, Vec<u8>)> {
        assert!(bytes.len() >= 6);
        assert_eq!(u16::from_le_bytes(bytes[0..2].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(bytes[2..4].try_into().unwrap()), 1);
        let count = u16::from_le_bytes(bytes[4..6].try_into().unwrap()) as usize;
        let mut images = Vec::with_capacity(count);
        for index in 0..count {
            let at = 6 + index * 16;
            let entry = &bytes[at..at + 16];
            let listed = if entry[0] == 0 {
                256
            } else {
                u32::from(entry[0])
            };
            assert_eq!(entry[1], entry[0], "宽和高一起记");
            let nbytes = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
            let dib = &bytes[offset..offset + nbytes];
            assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
            let width = i32::from_le_bytes(dib[4..8].try_into().unwrap());
            let height = i32::from_le_bytes(dib[8..12].try_into().unwrap());
            assert_eq!(width as u32, listed);
            assert_eq!(height, width * 2, "高度含 AND 掩码");
            assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 32);
            let width = width as usize;
            let mut rgba = vec![0u8; width * width * 4];
            let xor = &dib[40..40 + width * width * 4];
            for y in 0..width {
                let src_y = width - 1 - y;
                for x in 0..width {
                    let s = (src_y * width + x) * 4;
                    let blue = xor[s];
                    let green = xor[s + 1];
                    let red = xor[s + 2];
                    let alpha = xor[s + 3];
                    let o = (y * width + x) * 4;
                    rgba[o..o + 4].copy_from_slice(&[red, green, blue, alpha]);
                }
            }
            images.push((listed, rgba));
        }
        images
    }
}
