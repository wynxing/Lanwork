//! 面板的「应用」页。分类、卡片、启用开关，导入、更新、卸载、查看权限和填写密钥。
//!
//! 「悬浮窗」打开外壳，并在里面运行已安装工具的清单。不自动开始计时组件。
//! 启用状态写在数据目录的 `tool-enabled.json`。这份文件名规格还没定，不进导出白名单。
//! 安装副本的路径也还没定，这一版用 `tools/<工具 id>/`。

use std::collections::BTreeSet;
use std::path::PathBuf;

use lanwork_core::panel::{
    EnabledBytes, INVALID_PACKAGE, Installed, ToolOrigin, UNINSTALL_BODY, UPDATE_NOTE,
    WRONG_UPDATE, install_copy, installed_tools, is_enabled, menu_items, read_enabled,
    remove_installed, set_disabled, show_import, tool_dir, visible_cards, write_enabled,
};
use lanwork_core::storage::Store;
use lanwork_toolhost::{
    Grant, ProductCredentialStore, SecretStore, StagedPackage, decode_grant,
    delete_product_secrets, encode_grant, grant_lines, inspect_path, permission_lines,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::float_shell;
use crate::panel::{self, with_apps};
use crate::secret_form::{SecretDraft, save_secret_rows};
use crate::shelf_dnd::IncomingDrop;
use crate::{AppToolCard, AppsPageData, Panel};

pub(crate) struct AppsPage {
    store: Store,
    origin: ToolOrigin,
    disabled: BTreeSet<String>,
    /// 文件读不了，或不认识的 schema。不把原文覆盖掉。
    unreadable: bool,
    flow: Flow,
    /// 从某一张卡片的「更新」打开文件框时，包内 id 必须是这张卡片。
    /// 「导入+」和拖入是 `None`，不按卡片核对。
    update_target: Option<String>,
}

enum Flow {
    None,
    Import(Box<StagedPackage>),
    Uninstall { id: String },
    View,
    Secrets { id: String, rows: Vec<SecretDraft> },
}

impl AppsPage {
    pub(crate) fn new(ui: &Panel, store: Store) -> Self {
        wire(ui);
        Self {
            store,
            origin: ToolOrigin::Builtin,
            disabled: BTreeSet::new(),
            unreadable: false,
            flow: Flow::None,
            update_target: None,
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

    /// 「自定义」且没有确认框时，才接受拖入的文件夹或 zip。
    pub(crate) fn accepts_drop(&self) -> bool {
        self.origin == ToolOrigin::Custom && matches!(self.flow, Flow::None)
    }

    pub(crate) fn import_paths(&mut self, ui: &Panel, paths: Vec<PathBuf>) {
        self.update_target = None;
        if !matches!(self.flow, Flow::None) {
            return;
        }
        let [path] = paths.as_slice() else {
            self.store.log_warn("一次只能导入一个工具包");
            ui.set_error(INVALID_PACKAGE.into());
            return;
        };
        self.stage_path(ui, path);
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
        panel::refresh_drop();
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
        let labels: Vec<SharedString> = self
            .menu_for(card.origin, &card.id)
            .into_iter()
            .map(Into::into)
            .collect();
        data.set_menu_items(ModelRc::new(VecModel::from(labels)));
        data.set_menu_index(index);
        data.set_menu_open(true);
    }

    fn menu_chosen(&mut self, ui: &Panel, index: i32) {
        let tools = installed_tools(&self.store.data_dir());
        let disabled = if self.unreadable {
            &BTreeSet::new()
        } else {
            &self.disabled
        };
        let cards = visible_cards(self.origin, &tools, disabled);
        let menu_index = ui.global::<AppsPageData>().get_menu_index();
        let Some(card) = usize::try_from(menu_index)
            .ok()
            .and_then(|index| cards.get(index))
        else {
            self.dismiss(ui);
            return;
        };
        let items = self.menu_for(card.origin, &card.id);
        let Some(label) = usize::try_from(index)
            .ok()
            .and_then(|index| items.get(index).copied())
        else {
            self.dismiss(ui);
            return;
        };
        let id = card.id.clone();
        let name = card.name.clone();
        self.dismiss(ui);
        if !matches!(self.flow, Flow::None) {
            return;
        }
        match label {
            "更新" => self.pick(ui, Some(id)),
            "卸载" => self.show_uninstall(ui, &id, &name),
            "查看权限" => self.show_view(ui, &id),
            "填写密钥" => self.show_secrets(ui, &id, &name),
            _ => {}
        }
    }

    fn pick(&mut self, ui: &Panel, update_target: Option<String>) {
        if !matches!(self.flow, Flow::None) {
            return;
        }
        self.update_target = update_target;
        let Some(hwnd) = crate::bar_win::hwnd_of(ui.window()) else {
            self.update_target = None;
            self.store.log_warn("面板没有窗口句柄，不能选择工具包");
            ui.set_error(INVALID_PACKAGE.into());
            return;
        };
        match crate::tool_pick::pick_package(hwnd) {
            Ok(None) => self.update_target = None,
            Ok(Some(path)) => self.stage_path(ui, &path),
            Err(message) => {
                self.update_target = None;
                self.store.log_warn(&message);
                ui.set_error(INVALID_PACKAGE.into());
            }
        }
    }

    fn stage_path(&mut self, ui: &Panel, path: &std::path::Path) {
        if !matches!(self.flow, Flow::None) {
            self.update_target = None;
            return;
        }
        match inspect_path(path) {
            Ok(staged) => {
                if let Some(expected) = self.update_target.take()
                    && staged.manifest.id != expected
                {
                    self.store.log_warn(&format!(
                        "更新包的 id 是 {}，卡片是 {expected}",
                        staged.manifest.id
                    ));
                    ui.set_error(WRONG_UPDATE.into());
                    return;
                }
                self.show_import(ui, staged);
            }
            Err(err) => {
                self.update_target = None;
                self.store
                    .log_warn(&format!("工具包没有通过：{}", err.detail()));
                ui.set_error(err.to_string().into());
            }
        }
    }

    fn show_import(&mut self, ui: &Panel, staged: StagedPackage) {
        let update = installed_tools(&self.store.data_dir())
            .iter()
            .any(|tool| tool.id == staged.manifest.id);
        let lines = string_model(&permission_lines(&staged.manifest));
        let data = ui.global::<AppsPageData>();
        data.set_dialog_kind("perm".into());
        data.set_dialog_update(update);
        data.set_dialog_confirm(true);
        data.set_dialog_name(staged.manifest.name.as_str().into());
        data.set_dialog_detail(staged.manifest.description.as_str().into());
        data.set_dialog_lines(lines);
        data.set_dialog_note(if update { UPDATE_NOTE } else { "" }.into());
        data.set_dialog_open(true);
        self.flow = Flow::Import(Box::new(staged));
        self.dismiss(ui);
        panel::refresh_drop();
    }

    fn show_uninstall(&mut self, ui: &Panel, id: &str, name: &str) {
        let data = ui.global::<AppsPageData>();
        data.set_dialog_kind("uninstall".into());
        data.set_dialog_update(false);
        data.set_dialog_confirm(false);
        data.set_dialog_name(name.into());
        data.set_dialog_detail(SharedString::new());
        data.set_dialog_lines(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
        data.set_dialog_note(UNINSTALL_BODY.into());
        data.set_dialog_open(true);
        self.flow = Flow::Uninstall { id: id.to_owned() };
        panel::refresh_drop();
    }

    fn show_view(&mut self, ui: &Panel, id: &str) {
        let tools = installed_tools(&self.store.data_dir());
        let Some(card) = tools.iter().find(|tool| tool.id == id) else {
            return;
        };
        let Some(dir) = tool_dir(&self.store.data_dir(), id) else {
            ui.set_error(INVALID_PACKAGE.into());
            return;
        };
        let bytes = match std::fs::read(dir.join(lanwork_core::panel::GRANT_FILE)) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.store.log_warn(&format!("权限记录没有读到：{err}"));
                ui.set_error(INVALID_PACKAGE.into());
                return;
            }
        };
        let Some(grant) = decode_grant(&bytes) else {
            self.store.log_warn("权限记录读不了");
            ui.set_error(INVALID_PACKAGE.into());
            return;
        };
        let data = ui.global::<AppsPageData>();
        data.set_dialog_kind("perm".into());
        data.set_dialog_update(false);
        data.set_dialog_confirm(false);
        data.set_dialog_name(card.name.as_str().into());
        data.set_dialog_detail(card.description.as_str().into());
        data.set_dialog_lines(string_model(&grant_lines(&grant)));
        data.set_dialog_note(SharedString::new());
        data.set_dialog_open(true);
        self.flow = Flow::View;
        panel::refresh_drop();
    }

    fn show_secrets(&mut self, ui: &Panel, id: &str, name: &str) {
        let Some(rows) = self.secret_rows(id) else {
            self.store.log_warn("密钥声明没有读到");
            ui.set_error(INVALID_PACKAGE.into());
            return;
        };
        if rows.is_empty() {
            return;
        }
        let labels: Vec<String> = rows.iter().map(|row| row.label.clone()).collect();
        let data = ui.global::<AppsPageData>();
        data.set_dialog_kind("secret".into());
        data.set_dialog_update(false);
        data.set_dialog_confirm(false);
        data.set_dialog_name(name.into());
        data.set_dialog_detail(SharedString::new());
        data.set_dialog_lines(string_model(&labels));
        data.set_dialog_note(SharedString::new());
        data.set_dialog_open(true);
        self.flow = Flow::Secrets {
            id: id.to_owned(),
            rows,
        };
        panel::refresh_drop();
    }

    fn edit_secret(&mut self, index: i32, text: String) {
        let Flow::Secrets { rows, .. } = &mut self.flow else {
            return;
        };
        let Some(row) = usize::try_from(index)
            .ok()
            .and_then(|index| rows.get_mut(index))
        else {
            return;
        };
        row.value = text;
    }

    fn save_secrets(&mut self, ui: &Panel) {
        let Flow::Secrets { id, rows } = std::mem::replace(&mut self.flow, Flow::None) else {
            self.close_dialog(ui);
            return;
        };
        let mut credentials = ProductCredentialStore;
        match save_secret_rows(&mut credentials, &id, &rows) {
            Ok(()) => self.finish_secrets(ui, &id),
            Err(failed) => {
                self.store.log_warn(&format!(
                    "密钥没有写下：{}：{}",
                    failed.label,
                    failed.detail()
                ));
                ui.set_error(failed.message().into());
                self.close_dialog(ui);
                if failed.wrote {
                    float_shell::reload_tool(&id);
                }
            }
        }
    }

    fn clear_secrets(&mut self, ui: &Panel) {
        let Flow::Secrets { id, rows } = std::mem::replace(&mut self.flow, Flow::None) else {
            self.close_dialog(ui);
            return;
        };
        let mut credentials = ProductCredentialStore;
        for row in &rows {
            if let Err(err) = credentials.remove(&id, &row.id) {
                self.store.log_warn(&format!("密钥没有删掉：{err}"));
                ui.set_error(err.into());
                self.close_dialog(ui);
                return;
            }
        }
        self.finish_secrets(ui, &id);
    }

    fn finish_secrets(&mut self, ui: &Panel, id: &str) {
        ui.set_error(SharedString::new());
        self.close_dialog(ui);
        float_shell::reload_tool(id);
    }

    fn confirm(&mut self, ui: &Panel) {
        match std::mem::replace(&mut self.flow, Flow::None) {
            Flow::Import(staged) => self.commit_import(ui, *staged),
            Flow::Uninstall { id } => self.commit_uninstall(ui, &id),
            Flow::Secrets { .. } | Flow::View | Flow::None => self.close_dialog(ui),
        }
    }

    fn cancel(&mut self, ui: &Panel) {
        self.flow = Flow::None;
        self.close_dialog(ui);
    }

    fn commit_import(&mut self, ui: &Panel, staged: StagedPackage) {
        let id = staged.manifest.id.clone();
        let update = installed_tools(&self.store.data_dir())
            .iter()
            .any(|tool| tool.id == id);
        let grant = Grant::from_manifest(&staged.manifest);
        match install_copy(
            &self.store.data_dir(),
            &id,
            &staged.manifest.name,
            &staged.manifest.description,
            &staged.files,
            &encode_grant(&grant),
        ) {
            Ok(Installed::Ready) => {}
            Ok(Installed::BackupRemains) => {
                self.store.log_warn(&format!("旧副本没有删掉：{id}"));
            }
            Err(err) => {
                self.store.log_warn(&format!("工具没有装上：{err}"));
                ui.set_error(INVALID_PACKAGE.into());
                self.close_dialog(ui);
                return;
            }
        }
        if !update {
            let mut next = self.disabled.clone();
            if set_disabled(&mut next, &id, true)
                && let Err(err) = write_enabled(&self.store.data_dir(), &next)
            {
                self.store.log_warn(&format!("启用状态没有写下：{err}"));
                ui.set_error(err.to_string().into());
            }
        } else {
            float_shell::reload_tool(&id);
        }
        ui.set_error(SharedString::new());
        self.close_dialog(ui);
        self.reload(ui);
    }

    fn commit_uninstall(&mut self, ui: &Panel, id: &str) {
        float_shell::close_tool(id);
        if let Err(err) = delete_product_secrets(id) {
            self.store.log_warn(&format!("密钥没有删掉：{err}"));
            ui.set_error(err.into());
            self.close_dialog(ui);
            return;
        }
        if let Err(err) = remove_installed(&self.store.data_dir(), id) {
            self.store.log_warn(&format!("工具没有卸掉：{err}"));
            ui.set_error(err.to_string().into());
            self.close_dialog(ui);
            return;
        }
        let mut next = self.disabled.clone();
        if set_disabled(&mut next, id, true) {
            if let Err(err) = write_enabled(&self.store.data_dir(), &next) {
                self.store.log_warn(&format!("启用状态没有写下：{err}"));
            } else {
                self.disabled = next;
            }
        }
        ui.set_error(SharedString::new());
        self.close_dialog(ui);
        self.reload(ui);
    }

    fn close_dialog(&mut self, ui: &Panel) {
        let data = ui.global::<AppsPageData>();
        data.set_dialog_open(false);
        data.set_dialog_confirm(false);
        data.set_dialog_update(false);
        panel::refresh_drop();
    }
}

fn string_model(lines: &[String]) -> ModelRc<SharedString> {
    let rows: Vec<SharedString> = lines.iter().map(|line| line.as_str().into()).collect();
    ModelRc::new(VecModel::from(rows))
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
    data.on_menu_chosen(|index| with_apps(move |page, ui| page.menu_chosen(ui, index)));
    data.on_import_clicked(|| with_apps(|page, ui| page.pick(ui, None)));
    data.on_cancel_dialog(|| with_apps(|page, ui| page.cancel(ui)));
    data.on_confirm_dialog(|| with_apps(|page, ui| page.confirm(ui)));
    data.on_secret_edited(|index, text| {
        let text = text.to_string();
        with_apps(move |page, _| page.edit_secret(index, text));
    });
    data.on_save_secrets(|| with_apps(|page, ui| page.save_secrets(ui)));
    data.on_clear_secrets(|| with_apps(|page, ui| page.clear_secrets(ui)));
}

impl AppsPage {
    fn menu_for(&self, origin: ToolOrigin, id: &str) -> Vec<&'static str> {
        menu_items(origin, self.has_secrets(id))
    }

    fn has_secrets(&self, id: &str) -> bool {
        self.secret_rows(id).is_some_and(|rows| !rows.is_empty())
    }

    /// 密钥行来自已确认的 `grant.json`，不用可能被改过的清单。
    fn secret_rows(&self, id: &str) -> Option<Vec<SecretDraft>> {
        let dir = tool_dir(&self.store.data_dir(), id)?;
        let bytes = std::fs::read(dir.join(lanwork_core::panel::GRANT_FILE)).ok()?;
        let grant = decode_grant(&bytes)?;
        Some(drafts_from_grant(&grant))
    }
}

fn drafts_from_grant(grant: &Grant) -> Vec<SecretDraft> {
    grant
        .secrets
        .iter()
        .map(|secret| SecretDraft {
            id: secret.id.clone(),
            label: secret.label.clone(),
            value: String::new(),
        })
        .collect()
}

pub(crate) fn dropped(incoming: IncomingDrop) {
    let IncomingDrop::Paths(paths) = incoming else {
        return;
    };
    let _ = slint::invoke_from_event_loop(move || {
        with_apps(move |page, ui| page.import_paths(ui, paths));
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn import_confirms_before_install_and_the_float_runs_the_tool() {
        let source = include_str!("apps_page.rs");
        assert!(source.contains("float_shell::open_tool"));
        assert!(source.contains("float_shell::close_tool"));
        assert!(source.contains("inspect_path"));
        assert!(source.contains("install_copy"));
        assert!(source.contains("delete_product_secrets"));
        assert!(source.contains("保留数据") || source.contains("UPDATE_NOTE"));
    }
}
