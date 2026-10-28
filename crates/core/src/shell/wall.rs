//! 应用墙的格子。不创建窗口。
//!
//! 列数、每格的位置和当前该取图标的下标在这里算。宽高和滚动量由界面传入。
//! 这些像素是实现选择，产品规格没有写。

/// 一格的宽，逻辑像素。图标在格内居中，名称在图标下面。
pub const CELL_W: u32 = 112;
/// 一格的高，逻辑像素。
pub const CELL_H: u32 = 104;
/// 相邻格子的间隔，逻辑像素。
pub const GAP: u32 = 12;
/// 网格左右留白。列数按去掉这份留白之后能放下几列来算，整网格再在窗口里居中。
pub const PAD_X: u32 = 36;
/// 网格内容上下留白。
pub const PAD_Y: u32 = 8;

/// 一次排出来的网格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallGrid {
    pub columns: u32,
    pub origin_x: u32,
    pub content_height: u32,
    pub cell_w: u32,
    pub cell_h: u32,
}

/// 按窗口逻辑宽度和条目数排网格。宽度比一格还窄时仍是一列。
#[must_use]
pub fn grid(width: u32, count: usize) -> WallGrid {
    let columns = columns_for(width);
    let grid_w = columns * CELL_W + columns.saturating_sub(1) * GAP;
    let origin_x = width.saturating_sub(grid_w) / 2;
    let rows = if count == 0 || columns == 0 {
        0
    } else {
        count.div_ceil(columns as usize)
    };
    let content_height = if rows == 0 {
        0
    } else {
        PAD_Y * 2 + (rows as u32) * CELL_H + (rows as u32 - 1) * GAP
    };
    WallGrid {
        columns,
        origin_x,
        content_height,
        cell_w: CELL_W,
        cell_h: CELL_H,
    }
}

/// 第 `index` 格的左上角，相对网格内容。
#[must_use]
pub fn tile_xy(grid: &WallGrid, index: usize) -> (u32, u32) {
    let columns = grid.columns.max(1) as usize;
    let column = index % columns;
    let row = index / columns;
    let x = grid.origin_x + (column as u32) * (CELL_W + GAP);
    let y = PAD_Y + (row as u32) * (CELL_H + GAP);
    (x, y)
}

/// 视口里的格子，再多留一行，用来提前取图标。`scroll_top` 和 `viewport` 是网格内容的逻辑像素。
#[must_use]
pub fn visible_indices(
    grid: &WallGrid,
    count: usize,
    scroll_top: u32,
    viewport: u32,
) -> std::ops::Range<usize> {
    if count == 0 || viewport == 0 {
        return 0..0;
    }
    let stride = CELL_H + GAP;
    let first_row = scroll_top.saturating_sub(PAD_Y) / stride;
    let bottom = scroll_top.saturating_add(viewport).saturating_add(stride);
    let last_row = bottom.saturating_sub(PAD_Y) / stride;
    let columns = grid.columns.max(1) as usize;
    let start = (first_row as usize).saturating_mul(columns).min(count);
    let end = (last_row as usize)
        .saturating_add(1)
        .saturating_mul(columns)
        .min(count);
    start..end.max(start)
}

fn columns_for(width: u32) -> u32 {
    let usable = width.saturating_sub(PAD_X.saturating_mul(2));
    let stride = CELL_W + GAP;
    if usable < CELL_W {
        1
    } else {
        ((usable + GAP) / stride).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::{CELL_H, CELL_W, GAP, PAD_X, PAD_Y, grid, tile_xy, visible_indices};

    #[test]
    fn three_columns_center_in_the_window_and_stack_downward() {
        let width = PAD_X * 2 + CELL_W * 3 + GAP * 2;
        let laid = grid(width, 4);
        assert_eq!(laid.columns, 3);
        assert_eq!(laid.origin_x, PAD_X);
        assert_eq!(tile_xy(&laid, 0), (PAD_X, PAD_Y));
        assert_eq!(tile_xy(&laid, 1), (PAD_X + CELL_W + GAP, PAD_Y));
        assert_eq!(tile_xy(&laid, 3), (PAD_X, PAD_Y + CELL_H + GAP));
        assert_eq!(laid.content_height, PAD_Y * 2 + CELL_H * 2 + GAP);
    }

    #[test]
    fn a_narrow_window_is_one_column() {
        let laid = grid(CELL_W, 2);
        assert_eq!(laid.columns, 1);
        assert_eq!(tile_xy(&laid, 1).1, PAD_Y + CELL_H + GAP);
    }

    #[test]
    fn visible_range_keeps_one_extra_row() {
        let laid = grid(PAD_X * 2 + CELL_W * 3 + GAP * 2, 10);
        let visible = visible_indices(&laid, 10, 0, 200);
        assert_eq!(visible, 0..9);
        assert!(visible_indices(&laid, 10, 0, 0).is_empty());
        assert!(visible_indices(&laid, 0, 0, 200).is_empty());
    }
}
