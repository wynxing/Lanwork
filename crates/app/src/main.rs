// 命令层将调用 lanwork-core 的服务。当前还没有命令。
use lanwork_core as _;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    MainWindow::new()?.run()
}
