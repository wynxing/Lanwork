//! 工具的宿主。没有界面。
//!
//! 沙箱、清单和权限服务在这里。悬浮窗的软件渲染验证仍在 `spikes/toolhost`。
//! 技术验证「工具宿主」已通过，见 `docs/roadmap.md`。
//! 凭据管理器界面待用户用 `lanwork --toolhost-cred` 确认。
//!
//! 这个 crate 不打开数据目录，也不读写 MayDolist。待办、安静时段和密钥库由调用方注入。
//! 内存和 CPU 的数字上限仍待定。

mod guide;
mod host;
mod http;
mod id;
pub mod login;
mod manifest;
mod package;
mod pattern;
mod secret;
mod usage;

pub mod probe;

pub use guide::{quota, write_example};
pub use host::{
    Notice, Notification, Options, Session, StoragePersist, TodoComplete, TodoItem, WidgetView,
    WidgetViewKind,
};
pub use http::{
    HttpResponse, HttpTransport, Outgoing, RustlsTransport, TransportError, install_ring,
};
pub use id::{imported_prefix_forbidden, valid_id};
pub use manifest::{Manifest, load_dir, materialize};
pub use package::{
    CopyPreview, FOLDER_SENTINEL, Grant, MAX_ENTRIES as TOOL_MAX_FILES,
    MAX_ENTRY as TOOL_MAX_ENTRY, MAX_UNCOMPRESSED as TOOL_MAX_BYTES, PackageError, PickedSource,
    RecheckedTool, StagedPackage, decode_grant, encode_grant, grant_lines, inspect_path,
    inspect_zip_bytes, permission_lines, preview_copy, recheck_installed, resolve_selection,
};
pub use secret::{
    MemorySecretStore, SecretStore, delete_product_secrets, product_target_name, spike_target_name,
};
pub use usage::{UsageRoots, UsageSource};

#[cfg(windows)]
pub use secret::{ProductCredentialStore, WindowsCredentialStore};

#[cfg(test)]
mod tests;
