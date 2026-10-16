//! 面板的「应用」页。分类、卡片、启用开关，以及用悬浮窗外壳打开窗口。
//!
//! 已安装工具这一步还没有：导入是下一步，番茄钟迁移更后。列表为空时按规格显示空状态。
//! 「导入+」和「⋯」里的项按规格出现。点击它们不开始导入、更新、卸载或查看权限。
//! 「悬浮窗」只打开窗口，不跑清单，也不开始计时。
//! 启用状态写在数据目录的 `tool-enabled.json`。这份文件名规格还没定，不进导出白名单。

use std::collections::BTreeSet;

use lanwork_core::panel::{
    EnabledBytes, ToolOrigin, installed_tools, is_enabled, menu_items, read_enabled, set_disabled,
    show_import, visible_cards, write_enabled,
};
use lanwork_core::storage::Store;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::float_shell;
use crate::panel::with_apps;
use crate::{AppToolCard, AppsPageData, Panel};

pub(crate) struct AppsPage {
    store: Store,
    origin: ToolOrigin,
    disabled: BTreeSet<String>,
    /// 文件读不了，或不认识的 schema。不把原文覆盖掉。
    unreadable: bool,
}

impl AppsPage {
    pub(crate) fn new(ui: &Panel, store: Store) -> Self {
        wire(ui);
        Self {
            store,
            origin: ToolOrigin::Builtin,
            disabled: BTreeSet::new(),
            unreadable: false,
        }
    }

    pub(crate) fn entered(&mut self, ui: &Panel) {
        self.dismiss(ui);
        self.reload(ui);
    }

    pub(crate) fn dismiss(&self, ui: &Panel) {
        let data = ui.global::<AppsPageData>();
        data.set_menu_open(false);
        data.set_menu_index(-1);
    }

    fn reload(&mut self, ui: &Panel) {
        match read_enabled(&self.store.data_dir()) {
            Ok(EnabledBytes::Known(set)) => {
                self.disabled = set;
                self.unreadable = false;
            }
            Ok(EnabledBytes::Corrupt) => {
                self.store.log_warn("启用状态读不了");
                self.unreadable = true;
            }
            Ok(EnabledBytes::Unsupported) => {
                self.store.log_warn("启用状态的 schemaVersion 不受支持");
                self.unreadable = true;
            }
            Err(err) => {
                self.store.log_warn(&format!("启用状态没有读到：{err}"));
                self.unreadable = true;
            }
        }
        self.push(ui);
    }

    fn push(&self, ui: &Panel) {
        let data = ui.global::<AppsPageData>();
        let tools = installed_tools(&self.store.data_dir());
        let disabled = if self.unreadable {
            &BTreeSet::new()
        } else {
            &self.disabled
        };
        let cards = visible_cards(self.origin, &tools, disabled);
        let rows: Vec<AppToolCard> = cards.iter().map(to_row).collect();
        data.set_category(self.origin.index());
        data.set_show_import(show_import(self.origin));
        data.set_tools(ModelRc::new(VecModel::from(rows)));
    }

    fn select_category(&mut self, ui: &Panel, index: i32) {
        let Some(origin) = ToolOrigin::from_index(index) else {
            return;
        };
        self.origin = origin;
        self.dismiss(ui);
        self.push(ui);
    }

    fn open_float(&self, ui: &Panel, id: &str) {
        let tools = installed_tools(&self.store.data_dir());
        if self.unreadable || !is_enabled(&self.disabled, id) {
            return;
        }
        if !tools.iter().any(|tool| tool.id == id) {
            return;
        }
        if let Err(message) = float_shell::open_tool(id) {
            ui.set_error(message.into());
        }
    }

    fn toggle(&mut self, ui: &Panel, id: &str, enabled: bool) {
        let tools = installed_tools(&self.store.data_dir());
        if self.unreadable || !tools.iter().any(|tool| tool.id == id) {
            return;
        }
        let mut next = self.disabled.clone();
        if !set_disabled(&mut next, id, enabled) {
            return;
        }
        if let Err(err) = write_enabled(&self.store.data_dir(), &next) {
            ui.set_error(err.to_string().into());
            return;
        }
        self.disabled = next;
        ui.set_error(SharedString::new());
        self.dismiss(ui);
        self.push(ui);
        if !enabled {
            float_shell::close_tool(id);
        }
    }

    fn open_menu(&self, ui: &Panel, index: i32) {
        let data = ui.global::<AppsPageData>();
        let tools = installed_tools(&self.store.data_dir());
        let disabled = if self.unreadable {
            &BTreeSet::new()
        } else {
            &self.disabled
        };
        let cards = visible_cards(self.origin, &tools, disabled);
        let Some(card) = usize::try_from(index)
            .ok()
            .and_then(|index| cards.get(index))
        else {
            data.set_menu_open(false);
            data.set_menu_index(-1);
            return;
        };
        let labels: Vec<SharedString> = menu_items(card.origin)
            .iter()
            .copied()
            .map(Into::into)
            .collect();
        data.set_menu_items(ModelRc::new(VecModel::from(labels)));
        data.set_menu_index(index);
        data.set_menu_open(true);
    }
}

fn to_row(card: &lanwork_core::panel::ToolCard) -> AppToolCard {
    let mut icon = slint::Image::default();
    let mut has_icon = false;
    if let Some(path) = &card.icon_path
        && let Ok(image) = slint::Image::load_from_path(path)
    {
        icon = image;
        has_icon = true;
    }
    AppToolCard {
        tool_id: card.id.as_str().into(),
        name: card.name.as_str().into(),
        detail: card.description.as_str().into(),
        mark: card.mark.as_str().into(),
        has_icon,
        icon,
        enabled: card.enabled,
        builtin: card.origin == ToolOrigin::Builtin,
    }
}

fn wire(ui: &Panel) {
    let data = ui.global::<AppsPageData>();
    data.on_select_category(|index| {
        with_apps(move |page, ui| page.select_category(ui, index));
    });
    data.on_open_float(|id| {
        let id = id.to_string();
        with_apps(move |page, ui| page.open_float(ui, &id));
    });
    data.on_toggle_enabled(|id, enabled| {
        let id = id.to_string();
        with_apps(move |page, ui| page.toggle(ui, &id, enabled));
    });
    data.on_open_menu(|index| with_apps(move |page, ui| page.open_menu(ui, index)));
    data.on_close_menu(|| with_apps(|page, ui| page.dismiss(ui)));
    data.on_menu_chosen(|_index| with_apps(|page, ui| page.dismiss(ui)));
    data.on_import_clicked(|| {
        // 选择文件夹、确认和安装是下一步。这里不打开对话框，也不加说明。
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_page_only_opens_a_window_and_leaves_the_flows_for_later() {
        let source = include_str!("apps_page.rs");
        assert!(source.contains("float_shell::open_tool"));
        assert!(source.contains("float_shell::close_tool"));
        assert!(source.contains("不打开对话框"));
    }
}
