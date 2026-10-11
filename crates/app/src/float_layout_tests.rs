//! 收纳悬浮窗的布局检查。用软件渲染的窗口把 `ShelfFloat` 摆出来，读它导出的各块位置，
//! 确认「关联待办」标题行、关联列表、条目列表和底部操作行在展开、收起、不同条数下互不重叠。
//!
//! 起因：标题行当时没写 `y`，Slint 会把没有 `x`、`y` 的子元素放在父元素正中，
//! 展开后父元素变高，标题行被推到列表的第一行上。

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{ShelfFloat, ShelfLinkRow, ShelfRefRow, ShelvesPageData};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const WINDOW_HEIGHT: u32 = 360;
const TITLE_STRIP: f32 = 28.0;
const HEAD: f32 = 28.0;
const ROW: f32 = 30.0;
const MAX_ROWS: usize = 4;

fn links(count: usize) -> Vec<ShelfLinkRow> {
    (0..count)
        .map(|index| ShelfLinkRow {
            title: format!("待办 {index}").into(),
            found: true,
            completed: index % 2 == 0,
            in_trash: false,
        })
        .collect()
}

fn lay_out(window: &Rc<MinimalSoftwareWindow>, open: bool, count: usize, selected: bool) {
    let ui = ShelfFloat::new().expect("window");
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));
    let data = ui.global::<ShelvesPageData>();
    data.set_links(ModelRc::new(VecModel::from(links(count))));
    data.set_links_open(open);
    data.set_refs(ModelRc::new(VecModel::from(vec![ShelfRefRow {
        name: "你好.txt".into(),
        icon: slint::Image::default(),
        has_icon: false,
        missing: false,
    }])));
    data.set_selected_ref(if selected { 0 } else { -1 });
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();

    let label = format!("open={open} links={count} selected={selected}");
    let (head_y, head_h) = (ui.get_links_head_y(), ui.get_links_head_height());
    let (body_y, body_h) = (ui.get_links_body_y(), ui.get_links_body_height());
    let (refs_y, refs_h) = (ui.get_refs_area_y(), ui.get_refs_area_height());
    let action_y = ui.get_action_y();

    assert_eq!(head_y, TITLE_STRIP, "{label}: 标题行紧挨着窗口上沿的拖动条");
    assert_eq!(head_h, HEAD, "{label}");
    assert_eq!(body_y, head_y + head_h, "{label}: 关联列表紧接在标题行下面");
    let rows = if open { count.clamp(1, MAX_ROWS) } else { 0 };
    #[allow(clippy::cast_precision_loss)]
    let expected_body = rows as f32 * ROW;
    assert_eq!(body_h, expected_body, "{label}: 关联列表高度");
    assert_eq!(
        refs_y,
        body_y + body_h,
        "{label}: 条目列表紧接在关联列表下面"
    );
    assert!(refs_h > 0.0, "{label}: 条目列表还有空间");
    if selected {
        assert!(
            refs_y + refs_h <= action_y,
            "{label}: 条目列表不压到底部操作行 ({} > {action_y})",
            refs_y + refs_h
        );
    }
    #[allow(clippy::cast_precision_loss)]
    let bottom = WINDOW_HEIGHT as f32;
    assert!(refs_y + refs_h <= bottom, "{label}: 不超出窗口");
}

#[test]
fn the_float_window_blocks_never_overlap() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    for open in [false, true] {
        for count in [0, 1, 2, 4, 5, 9] {
            for selected in [false, true] {
                lay_out(&window, open, count, selected);
            }
        }
    }
}
