//! DLL 先找程序目录，开发构建再找仓库里的 `third_party/everything/`。

use std::path::{Path, PathBuf};

pub(crate) struct DllRoots {
    program_dir: Option<PathBuf>,
    dev_root: Option<PathBuf>,
}

impl DllRoots {
    pub(crate) fn from_process() -> Self {
        let program_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf));
        Self {
            program_dir,
            dev_root: dev_root(),
        }
    }

    pub(crate) fn sdk3(&self) -> Result<PathBuf, &'static str> {
        self.find("Everything3_x64.dll", "sdk3/Everything3_x64.dll")
    }

    pub(crate) fn sdk14(&self) -> Result<PathBuf, &'static str> {
        self.find("Everything64.dll", "sdk/Everything64.dll")
    }

    fn find(
        &self,
        program_name: &'static str,
        dev_relative: &str,
    ) -> Result<PathBuf, &'static str> {
        if let Some(dir) = &self.program_dir {
            let path = dir.join(program_name);
            if path.is_file() {
                return Ok(path);
            }
        }
        if let Some(root) = &self.dev_root {
            let path = root.join(dev_relative);
            if path.is_file() {
                return Ok(path);
            }
        }
        Err(program_name)
    }
}

fn dev_root() -> Option<PathBuf> {
    // 发布构建只认程序目录。开发构建才能回到仓库，避免把编译机路径写进发布包。
    #[cfg(debug_assertions)]
    {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/everything");
        root.is_dir().then_some(root)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn debug_build_finds_the_bundled_dlls() {
        let roots = DllRoots::from_process();
        assert!(roots.sdk3().is_ok());
        assert!(roots.sdk14().is_ok());
    }
}
