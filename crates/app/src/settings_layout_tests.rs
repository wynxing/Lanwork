//! 设置页的布局和点击检查。用软件渲染的窗口把 `Panel` 的设置标签摆出来，点控件，
//! 确认它们各自发出请求，并且控件只显示 Rust 推来的值，不自己翻转。
//! 坐标与 `settingspage.slint` 开头的注释一致。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::software_renderer::RepaintBufferType;
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    ImportLine, Panel, SettingsBackup, SettingsExternalClient, SettingsHiddenApp,
    SettingsLayoutBox, SettingsPageData,
};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const WIDTH: u32 = 760;
const HEIGHT: u32 = 560;
const PAGE_Y: f32 = 49.0;
const NAV_X: f32 = 40.0;
const CONTROL_X: f32 = 132.0 + 16.0 + 120.0;

fn open(dark: bool) -> (Panel, Rc<MinimalSoftwareWindow>) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    let ui = Panel::new().expect("panel");
    ui.set_dark(dark);
    ui.set_tab(4);
    let data = ui.global::<SettingsPageData>();
    let titles = |items: &[&str]| {
        let titles: Vec<slint::SharedString> = items.iter().copied().map(Into::into).collect();
        ModelRc::new(VecModel::from(titles))
    };
    data.set_corner_titles(titles(&["右上", "左上", "左下", "右下", "关闭"]));
    data.set_theme_titles(titles(&["明", "暗", "跟随系统"]));
    data.set_engine_titles(titles(&["Google", "必应", "百度"]));
    data.set_hotkey("Ctrl+Alt+M".into());
    data.set_snooze_days("3".into());
    data.set_data_dir(r"C:\Users\me\Documents\Lanwork".into());
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    (ui, window)
}

fn click(ui: &Panel, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn section_center(index: f32) -> f32 {
    PAGE_Y + 8.0 + 40.0 * index + 18.0
}

fn row_center(index: f32) -> f32 {
    PAGE_Y + 8.0 + 48.0 * index + 24.0
}

fn open_section(ui: &Panel, index: f32) {
    click(ui, NAV_X, section_center(index));
}

#[test]
fn each_control_asks_for_its_own_change_and_does_not_flip_by_itself() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let startup = Rc::new(RefCell::new(Vec::new()));
    let corners = Rc::new(RefCell::new(Vec::new()));
    let recordings = Rc::new(Cell::new(0));
    let themes = Rc::new(RefCell::new(Vec::new()));
    let engines = Rc::new(RefCell::new(Vec::new()));
    {
        let startup = Rc::clone(&startup);
        data.on_set_launch_at_startup(move |value| startup.borrow_mut().push(value));
        let corners = Rc::clone(&corners);
        data.on_choose_corner(move |index| corners.borrow_mut().push(index));
        let recordings = Rc::clone(&recordings);
        data.on_toggle_recording(move || recordings.set(recordings.get() + 1));
        let themes = Rc::clone(&themes);
        data.on_choose_theme(move |index| themes.borrow_mut().push(index));
        let engines = Rc::clone(&engines);
        data.on_choose_engine(move |index| engines.borrow_mut().push(index));
    }

    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(
        *startup.borrow(),
        [true, true],
        "开关请求相反的值，值没变就还是同一个请求"
    );
    assert!(!data.get_launch_at_startup(), "开关不自己翻转");

    data.set_launch_at_startup(true);
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(startup.borrow().last(), Some(&false));

    click(&ui, CONTROL_X + 48.0 + 24.0, row_center(1.0));
    assert_eq!(*corners.borrow(), [1], "第二项是左上");
    assert_eq!(data.get_corner(), 0, "热角不自己换");

    click(&ui, CONTROL_X + 90.0, row_center(2.0));
    assert_eq!(recordings.get(), 1);

    // 「明」大约 33 逻辑像素宽，点第二项「暗」。
    click(&ui, CONTROL_X + 33.0 + 16.0, row_center(3.0));
    assert_eq!(*themes.borrow(), [1]);
    assert_eq!(data.get_theme(), 0, "主题不自己换");

    // 「Google」大约 62 逻辑像素宽，点第二项「必应」。
    click(&ui, CONTROL_X + 62.0 + 16.0, row_center(4.0));
    assert_eq!(*engines.borrow(), [1]);
    assert_eq!(data.get_engine(), 0, "搜索引擎不自己换");
}

#[test]
fn quiet_hours_and_the_other_sections_keep_their_own_controls() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let quiet = Rc::new(RefCell::new(Vec::new()));
    let startup = Rc::new(RefCell::new(Vec::new()));
    let sections = Rc::new(RefCell::new(Vec::new()));
    {
        let quiet = Rc::clone(&quiet);
        data.on_set_quiet(move |value| quiet.borrow_mut().push(value));
        let startup = Rc::clone(&startup);
        data.on_set_launch_at_startup(move |value| startup.borrow_mut().push(value));
        let sections = Rc::clone(&sections);
        data.on_choose_section(move |index| sections.borrow_mut().push(index));
    }

    open_section(&ui, 1.0);
    assert_eq!(*sections.borrow(), [1]);
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(*quiet.borrow(), [true, true]);
    assert!(!data.get_quiet_on(), "安静时段不自己打开");
    assert!(startup.borrow().is_empty(), "待办页点不到开机启动");

    data.set_quiet_on(true);
    data.set_quiet_start("22:00".into());
    data.set_quiet_end("08:00".into());
    slint::platform::update_timers_and_animations();
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(quiet.borrow().last(), Some(&false));

    open_section(&ui, 2.0);
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(quiet.borrow().len(), 3, "数据页没有安静时段开关");
    assert!(startup.borrow().is_empty());
}

#[test]
fn the_data_directory_button_asks_to_choose_and_does_not_change_the_path() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let chooses = Rc::new(Cell::new(0));
    {
        let chooses = Rc::clone(&chooses);
        data.on_choose_data_dir(move || chooses.set(chooses.get() + 1));
    }
    open_section(&ui, 2.0);
    let path = data.get_data_dir();
    // 行贴右。「更换」大约 52 逻辑像素宽，点它的中间。
    click(&ui, WIDTH as f32 - 16.0 - 26.0, row_center(0.0));
    assert_eq!(chooses.get(), 1);
    assert_eq!(data.get_data_dir(), path, "点更换不自己改路径");
    data.set_data_dir_enabled(false);
    click(&ui, WIDTH as f32 - 16.0 - 26.0, row_center(0.0));
    assert_eq!(chooses.get(), 1, "迁移进行中不再发出请求");
}

#[test]
fn backup_export_and_import_buttons_ask_and_do_not_run_when_disabled() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let backups = Rc::new(Cell::new(0));
    let exports = Rc::new(Cell::new(0));
    let imports = Rc::new(Cell::new(0));
    let confirms = Rc::new(Cell::new(0));
    let cancels = Rc::new(Cell::new(0));
    {
        let backups = Rc::clone(&backups);
        data.on_backup_now(move || backups.set(backups.get() + 1));
        let exports = Rc::clone(&exports);
        data.on_export_clicked(move || exports.set(exports.get() + 1));
        let imports = Rc::clone(&imports);
        data.on_import_clicked(move || imports.set(imports.get() + 1));
        let confirms = Rc::clone(&confirms);
        data.on_confirm_import(move || confirms.set(confirms.get() + 1));
        let cancels = Rc::clone(&cancels);
        data.on_cancel_import(move || cancels.set(cancels.get() + 1));
    }
    open_section(&ui, 2.0);
    let right = WIDTH as f32 - 16.0 - 20.0;
    click(&ui, right, row_center(1.0));
    click(&ui, right, row_center(2.0));
    click(&ui, right, row_center(3.0));
    assert_eq!(backups.get(), 1, "立即备份");
    assert_eq!(exports.get(), 1, "导出");
    assert_eq!(imports.get(), 1, "导入");
    assert_eq!(confirms.get(), 0);
    data.set_backup_enabled(false);
    data.set_export_enabled(false);
    data.set_import_enabled(false);
    slint::platform::update_timers_and_animations();
    click(&ui, right, row_center(1.0));
    click(&ui, right, row_center(2.0));
    click(&ui, right, row_center(3.0));
    assert_eq!(backups.get(), 1, "正在备份时不再发出请求");
    assert_eq!(exports.get(), 1);
    assert_eq!(imports.get(), 1);

    data.set_import_enabled(true);
    data.set_import_confirming(true);
    data.set_import_action("确认导入".into());
    data.set_import_overview("待办 1，便签 0，收纳 2".into());
    slint::platform::update_timers_and_animations();
    click(&ui, right, row_center(3.0));
    assert_eq!(confirms.get(), 1, "确认导入在行的右边");
    assert_eq!(imports.get(), 1, "确认时没有「导入」");
    // 「确认导入」大约 72 宽，左边隔 8 是「取消」。
    click(
        &ui,
        WIDTH as f32 - 16.0 - 72.0 - 8.0 - 23.0,
        row_center(3.0),
    );
    assert_eq!(cancels.get(), 1, "取消在确认的左边");

    let lines: Vec<ImportLine> = (0..12)
        .map(|index| ImportLine {
            text: format!("第 {index} 行").into(),
            heading: index % 3 == 0,
        })
        .collect();
    data.set_import_lines(ModelRc::new(VecModel::from(lines)));
    slint::platform::update_timers_and_animations();
    // 导入行下面才是权限列表。点第一行不确认。
    click(&ui, CONTROL_X, PAGE_Y + 8.0 + 48.0 * 4.0 + 11.0);
    assert_eq!(confirms.get(), 1, "权限列表不代替确认");
    click(&ui, right, row_center(3.0));
    assert_eq!(confirms.get(), 2, "工具多时确认仍在导入行右边");
}

#[test]
fn import_recovered_shifts_every_section_and_stays_a_line_of_text() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let chooses = Rc::new(Cell::new(0));
    {
        let chooses = Rc::clone(&chooses);
        data.on_choose_data_dir(move || chooses.set(chooses.get() + 1));
    }
    open_section(&ui, 2.0);
    let x = WIDTH as f32 - 16.0 - 26.0;
    click(&ui, x, row_center(0.0));
    assert_eq!(chooses.get(), 1);
    data.set_import_recovered(true);
    slint::platform::update_timers_and_animations();
    click(&ui, x, row_center(0.0));
    assert_eq!(chooses.get(), 1, "「导入未完成」占了原来第一行的位置");
    click(&ui, x, row_center(0.0) + 36.0);
    assert_eq!(chooses.get(), 2, "数据目录下移 36");
    open_section(&ui, 0.0);
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    let startup = Rc::new(RefCell::new(Vec::new()));
    {
        let startup = Rc::clone(&startup);
        data.on_set_launch_at_startup(move |value| startup.borrow_mut().push(value));
    }
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert!(startup.borrow().is_empty(), "常规页的开关也下移了");
    click(&ui, CONTROL_X + 20.0, row_center(0.0) + 36.0);
    assert_eq!(*startup.borrow(), [true]);
}

#[test]
fn the_update_button_sits_on_the_other_section() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let clicks = Rc::new(Cell::new(0));
    {
        let clicks = Rc::clone(&clicks);
        data.on_update_clicked(move || clicks.set(clicks.get() + 1));
    }
    assert_eq!(data.get_update_button().as_str(), "检查更新");
    // 右栏上边距 8 + 按钮 y 52 + 半高 15。
    let y = PAGE_Y + 8.0 + 52.0 + 15.0;
    click(&ui, CONTROL_X + 16.0, y);
    assert_eq!(clicks.get(), 0, "常规页没有检查更新");
    open_section(&ui, 3.0);
    click(&ui, CONTROL_X + 16.0, y);
    assert_eq!(clicks.get(), 1, "检查更新按钮能点到");
    data.set_update_enabled(false);
    click(&ui, CONTROL_X + 16.0, y);
    assert_eq!(clicks.get(), 1, "检查进行中不再发出请求");
}

#[test]
fn restore_asks_for_the_hidden_app_under_the_pointer() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    data.set_hidden_apps(ModelRc::new(VecModel::from(vec![SettingsHiddenApp {
        name: "记事本".into(),
    }])));
    slint::platform::update_timers_and_animations();
    let restored = Rc::new(RefCell::new(Vec::new()));
    {
        let restored = Rc::clone(&restored);
        data.on_restore_hidden(move |index| restored.borrow_mut().push(index));
    }
    open_section(&ui, 3.0);
    // 上边距 8、更新块 88、「外部 AI」一行 48、节标题 40，然后第一条右侧的「恢复」。
    let y = PAGE_Y + 8.0 + 88.0 + 48.0 + 40.0 + 22.0;
    click(&ui, WIDTH as f32 - 16.0 - 24.0, y);
    assert_eq!(*restored.borrow(), [0]);
}

#[test]
fn external_ai_switch_asks_and_does_not_flip_by_itself() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let requested = Rc::new(RefCell::new(Vec::new()));
    {
        let requested = Rc::clone(&requested);
        data.on_set_external_ai(move |enabled| requested.borrow_mut().push(enabled));
    }
    open_section(&ui, 3.0);
    // 版本块 88 之下那一行的开关中心。
    let y = PAGE_Y + 8.0 + 88.0 + 24.0;
    click(&ui, CONTROL_X + 20.0, y);
    assert_eq!(*requested.borrow(), [true]);
    assert!(!data.get_external_ai());
}

#[test]
fn backup_restore_sits_on_the_row_and_stays_quiet_while_confirming() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    data.set_backups(ModelRc::new(VecModel::from(vec![
        SettingsBackup {
            label: "手动 2026-10-16 00:00".into(),
        },
        SettingsBackup {
            label: "自动 2026-10-15".into(),
        },
    ])));
    slint::platform::update_timers_and_animations();
    let restored = Rc::new(RefCell::new(Vec::new()));
    {
        let restored = Rc::clone(&restored);
        data.on_restore_backup(move |index| restored.borrow_mut().push(index));
    }
    open_section(&ui, 2.0);
    let x = WIDTH as f32 - 16.0 - 24.0;
    let first = PAGE_Y + 8.0 + 48.0 * 4.0 + 22.0;
    click(&ui, x, first);
    click(&ui, x, first + 44.0);
    assert_eq!(*restored.borrow(), [0, 1]);
    data.set_import_confirming(true);
    slint::platform::update_timers_and_animations();
    click(&ui, x, first);
    assert_eq!(restored.borrow().len(), 2, "正在确认时不再开始另一份");
    data.set_import_confirming(false);
    data.set_backup_enabled(false);
    slint::platform::update_timers_and_animations();
    click(&ui, x, first);
    assert_eq!(restored.borrow().len(), 2, "备份进行中不再恢复");
}

fn draw(window: &Rc<MinimalSoftwareWindow>) {
    window.request_redraw();
    let _ = window.draw_if_needed(|renderer| {
        use slint::platform::software_renderer::Rgb565Pixel;
        let mut buffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
        renderer.render(&mut buffer, WIDTH as usize);
    });
}

fn pump(ui: &Panel, window: &Rc<MinimalSoftwareWindow>) {
    let data = ui.global::<SettingsPageData>();
    // 条件刚成立时元素还不在，changed 不会跑。先画一帧再改 tick，几何才会写出来。
    slint::platform::update_timers_and_animations();
    draw(window);
    data.set_layout_tick(data.get_layout_tick() + 1);
    slint::platform::update_timers_and_animations();
    draw(window);
    data.set_layout_tick(data.get_layout_tick() + 1);
    slint::platform::update_timers_and_animations();
    draw(window);
}

fn overlaps(a: &SettingsLayoutBox, b: &SettingsLayoutBox) -> bool {
    a.shown
        && b.shown
        && a.w > 0.0
        && a.h > 0.0
        && b.w > 0.0
        && b.h > 0.0
        && a.x < b.x + b.w
        && b.x < a.x + a.w
        && a.y < b.y + b.h
        && b.y < a.y + a.h
}

fn assert_apart(label: &str, a: &SettingsLayoutBox, b: &SettingsLayoutBox) {
    assert!(
        !overlaps(a, b),
        "{label} 重叠 ({}, {}, {}, {}) 与 ({}, {}, {}, {})",
        a.x,
        a.y,
        a.w,
        a.h,
        b.x,
        b.y,
        b.w,
        b.h
    );
}

fn assert_same_left(label: &str, item: &SettingsLayoutBox, row: &SettingsLayoutBox) {
    assert!(item.shown, "{label} 没有摆出来");
    assert_eq!(item.x, row.x, "{label} 左缘与这一列文字不一致");
    assert!(
        item.x + item.w <= row.x + row.w + 0.5,
        "{label} 超出右缘 {} > {}",
        item.x + item.w,
        row.x + row.w
    );
}

/// 1024 物理像素、150% 缩放，再减去面板两侧 24 的边距。常见的偏窄 Windows 宽度。
const NARROW_PANEL: f32 = 634.0;

fn load_external(
    ui: &Panel,
    window: &Rc<MinimalSoftwareWindow>,
    with_clients: bool,
    unavailable: bool,
) {
    let data = ui.global::<SettingsPageData>();
    data.set_external_ai(true);
    data.set_external_unavailable(unavailable);
    data.set_external_endpoint(
        "http://127.0.0.1:39217/mcp/clients/abcdefghijklmnopqrstuvwxyz".into(),
    );
    if with_clients {
        data.set_external_token(
            "lw_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        );
        data.set_external_clients(ModelRc::new(VecModel::from(vec![
            SettingsExternalClient {
                name: "一个很长的客户端名称用来确认不会压住状态和撤销".into(),
                status: "已连接".into(),
            },
            SettingsExternalClient {
                name: "Cursor".into(),
                status: "未连接".into(),
            },
        ])));
    } else {
        data.set_external_token("".into());
        data.set_external_clients(ModelRc::new(VecModel::from(
            Vec::<SettingsExternalClient>::new(),
        )));
    }
    pump(ui, window);
}

fn assert_external_section(
    ui: &Panel,
    window: &Rc<MinimalSoftwareWindow>,
    panel_width: f32,
    with_clients: bool,
    unavailable: bool,
) {
    ui.set_panel_width(panel_width);
    load_external(ui, window, with_clients, unavailable);
    let data = ui.global::<SettingsPageData>();
    let row = data.get_external_row();
    let switch = data.get_external_switch();
    let field = data.get_external_field();
    let auth = data.get_external_auth();
    let token = data.get_external_token_box();
    let endpoint = data.get_external_endpoint_box();
    let empty = data.get_external_empty();
    let name = data.get_external_client_name();
    let status = data.get_external_client_status();
    let revoke = data.get_external_revoke();
    let unavail = data.get_external_unavail();

    let content = panel_width - 132.0 - 32.0;
    assert!(row.shown, "外部 AI 行没有摆出来");
    assert!(
        (row.w - content).abs() < 1.0,
        "行宽 {}，这一列应是 {}",
        row.w,
        content
    );
    assert_eq!(switch.x, row.x + 120.0, "开关与其它控件左缘一致");
    assert_eq!(switch.w, 40.0);
    assert!(switch.h > 0.0);

    assert!(field.shown, "名称框没有摆出来");
    assert!(auth.shown, "授权按钮没有摆出来");
    assert_same_left("名称", &field, &row);
    assert_eq!(field.w, 180.0, "名称框宽度");
    assert_eq!(field.h, 30.0, "名称框与其它输入同高");
    assert_eq!(auth.h, field.h, "授权与名称同高");
    assert_eq!(auth.y, field.y, "授权与名称同一行");
    assert_eq!(auth.x, field.x + field.w + 8.0, "名称和授权间隔 8");
    assert!(auth.w > 20.0, "授权按钮没有宽度");
    assert_apart("名称和授权", &field, &auth);
    assert!(auth.x + auth.w <= row.x + row.w + 0.5, "授权超出右缘");
    assert!(field.y >= row.y + row.h - 0.5, "名称应在开关行下面");

    assert_same_left("端点", &endpoint, &row);
    assert!(endpoint.h > 0.0);
    assert_apart("名称和端点", &field, &endpoint);

    if with_clients {
        assert!(!empty.shown, "有客户端时不显示空状态");
        assert_same_left("token", &token, &row);
        assert_eq!(token.h, 30.0, "token 框与输入同高");
        assert!(
            (token.w - row.w).abs() < 1.0,
            "token 框应铺满这一列 {} != {}",
            token.w,
            row.w
        );
        assert_apart("名称和 token", &field, &token);
        assert_apart("token 和端点", &token, &endpoint);
        assert!(token.y >= field.y + field.h - 0.5);
        assert!(endpoint.y >= token.y + token.h - 0.5);

        assert_same_left("客户端名称", &name, &row);
        assert!(status.shown && revoke.shown);
        assert_eq!(name.y, status.y, "状态与名称同一行");
        assert_eq!(
            revoke.y + revoke.h / 2.0,
            name.y + name.h / 2.0,
            "撤销垂直居中"
        );
        assert!(
            name.x + name.w <= status.x + 0.5,
            "名称压住状态 {} > {}",
            name.x + name.w,
            status.x
        );
        assert!(
            status.x + status.w <= revoke.x + 0.5,
            "状态压住撤销 {} > {}",
            status.x + status.w,
            revoke.x
        );
        assert_apart("名称和状态", &name, &status);
        assert_apart("状态和撤销", &status, &revoke);
        assert_apart("名称和撤销", &name, &revoke);
        assert!(revoke.x + revoke.w <= row.x + row.w + 0.5, "撤销超出右缘");
        assert!(name.y >= endpoint.y + endpoint.h - 0.5);
        assert_apart("端点和客户端", &endpoint, &name);
    } else {
        assert!(!token.shown, "没有 token 时不显示原文");
        assert!(!name.shown, "没有客户端时不显示列表");
        assert_same_left("没有客户端", &empty, &row);
        assert!(empty.y >= endpoint.y + endpoint.h - 0.5);
        assert_apart("端点和空状态", &endpoint, &empty);
        assert_apart("名称和空状态", &field, &empty);
    }

    if unavailable {
        assert!(unavail.shown, "无法监听没有摆出来");
        assert!(
            unavail.x >= switch.x + switch.w - 0.5,
            "无法监听压住开关 {} < {}",
            unavail.x,
            switch.x + switch.w
        );
        assert!(
            unavail.x + unavail.w <= row.x + row.w + 0.5,
            "无法监听超出右缘"
        );
        assert!(unavail.y >= row.y - 0.5 && unavail.y + unavail.h <= row.y + row.h + 0.5);
        assert_apart("无法监听和开关", &unavail, &switch);
        assert_apart("无法监听和名称", &unavail, &field);
    } else {
        assert!(!unavail.shown);
    }
}

#[test]
fn external_ai_name_field_and_authorize_button_do_not_overlap() {
    let (ui, window) = open(false);
    open_section(&ui, 3.0);
    let data = ui.global::<SettingsPageData>();
    let authorizes = Rc::new(Cell::new(0));
    let revokes = Rc::new(RefCell::new(Vec::new()));
    {
        let authorizes = Rc::clone(&authorizes);
        data.on_authorize_external(move || authorizes.set(authorizes.get() + 1));
        let revokes = Rc::clone(&revokes);
        data.on_revoke_external(move |index| revokes.borrow_mut().push(index));
    }

    for width in [WIDTH as f32, NARROW_PANEL] {
        assert_external_section(&ui, &window, width, true, true);
        assert_external_section(&ui, &window, width, false, false);
        assert_external_section(&ui, &window, width, true, false);
    }

    // 点名称占位符不应点到盖在上面的「授权」。再点按钮本身才授权。
    let data = ui.global::<SettingsPageData>();
    let field = data.get_external_field();
    let auth = data.get_external_auth();
    click(&ui, field.x + 14.0, field.y + field.h / 2.0);
    assert_eq!(authorizes.get(), 0, "名称左侧被授权挡住了");
    click(&ui, auth.x + auth.w / 2.0, auth.y + auth.h / 2.0);
    assert_eq!(authorizes.get(), 1, "授权按钮能点到");

    assert_external_section(&ui, &window, WIDTH as f32, true, false);
    let data = ui.global::<SettingsPageData>();
    let revoke = data.get_external_revoke();
    click(&ui, revoke.x + revoke.w / 2.0, revoke.y + revoke.h / 2.0);
    assert_eq!(*revokes.borrow(), [0], "撤销点到第一行");
}
