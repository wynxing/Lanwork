//! 供拖出和人工拖入使用的临时文件。不碰用户文档目录。

use std::fs;
use std::path::{Path, PathBuf};

use crate::ole::{self, wide_len};

pub struct Samples {
    pub dir: PathBuf,
    pub readme: PathBuf,
    pub folder: PathBuf,
    pub shortcut: PathBuf,
    pub long_file: PathBuf,
}

impl Samples {
    pub fn create() -> Result<Self, String> {
        let dir = std::env::temp_dir().join("lanwork-dnd-spike");
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .map_err(|error| format!("clean {}: {error}", dir.display()))?;
        }
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let readme = dir.join("readme.txt");
        fs::write(&readme, "lanwork dnd spike\n").map_err(|error| error.to_string())?;
        let folder = dir.join("folder");
        fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
        fs::write(folder.join("inside.txt"), "inside\n").map_err(|error| error.to_string())?;
        let shortcut = dir.join("shortcut.lnk");
        ole::create_shortcut(&shortcut, &readme)?;
        let long_file = create_long_file(&dir)?;
        Ok(Self {
            dir,
            readme,
            folder,
            shortcut,
            long_file,
        })
    }

    pub fn rows(&self) -> Vec<PathBuf> {
        vec![
            self.readme.clone(),
            self.folder.clone(),
            self.shortcut.clone(),
            self.long_file.clone(),
        ]
    }
}

fn create_long_file(root: &Path) -> Result<PathBuf, String> {
    let mut extended = PathBuf::from(r"\\?\");
    extended.push(root);
    extended.push("long");
    let piece = "d".repeat(40);
    loop {
        let visible = extended.strip_prefix(r"\\?\").unwrap_or(extended.as_path());
        if wide_len(visible) > 230 {
            break;
        }
        extended.push(&piece);
    }
    fs::create_dir_all(&extended).map_err(|error| format!("long dir: {error}"))?;
    let file = extended.join("long.txt");
    fs::write(&file, b"long").map_err(|error| format!("long file: {error}"))?;
    let visible = file.strip_prefix(r"\\?\").unwrap_or(&file).to_path_buf();
    if wide_len(&visible) <= 260 {
        return Err(format!(
            "long path is only {} wide units: {}",
            wide_len(&visible),
            visible.display()
        ));
    }
    Ok(visible)
}
