//! 发布包程序目录里要带上的 Everything 文件。
//!
//! 运行时从程序目录加载 DLL。许可原文在 `third_party/everything/`。
//! 安装程序还没做，这里只列出要拷贝的源和目标文件名。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramDirectoryFile {
    /// 仓库内的源路径，分隔符是 `/`。
    pub source: &'static str,
    /// 程序目录里的文件名。
    pub file_name: &'static str,
}

pub const PROGRAM_DIRECTORY_FILES: &[ProgramDirectoryFile] = &[
    ProgramDirectoryFile {
        source: "third_party/everything/sdk3/Everything3_x64.dll",
        file_name: "Everything3_x64.dll",
    },
    ProgramDirectoryFile {
        source: "third_party/everything/sdk/Everything64.dll",
        file_name: "Everything64.dll",
    },
    ProgramDirectoryFile {
        source: "third_party/everything/sdk3/LICENSE.txt",
        file_name: "Everything-SDK3-LICENSE.txt",
    },
    ProgramDirectoryFile {
        source: "third_party/everything/sdk/LICENSE.txt",
        file_name: "Everything-SDK-LICENSE.txt",
    },
];

#[cfg(test)]
pub(crate) fn repo_source(relative: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_sources_exist_and_licenses_keep_the_notice() {
        assert_eq!(PROGRAM_DIRECTORY_FILES.len(), 4);
        for file in PROGRAM_DIRECTORY_FILES {
            let path = repo_source(file.source);
            let meta = std::fs::metadata(&path).unwrap_or_else(|err| {
                panic!("missing {}: {err}", path.display());
            });
            assert!(meta.len() > 0, "{}", file.source);
        }
        for source in [
            "third_party/everything/sdk/LICENSE.txt",
            "third_party/everything/sdk3/LICENSE.txt",
        ] {
            let text = std::fs::read_to_string(repo_source(source)).unwrap();
            assert!(text.contains("Copyright (C)"));
            assert!(text.contains("Permission is hereby granted"));
            assert!(
                text.contains("The above copyright notice and this permission notice shall be")
            );
        }
    }
}
