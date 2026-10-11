//! 安装标识、发布目录清单，以及开始菜单快捷方式的属性。
//!
//! 快捷方式的 COM 写入只在 Windows 上编译。标识和清单在各平台都能测。

use std::fs;
use std::path::{Path, PathBuf};

use lanwork_core::files::PROGRAM_DIRECTORY_FILES;

/// 安装版 AUMID。和 spike 的 `Lanwork.Spike.Toast` 分开，也和 Run 值名相同。
pub const AUMID: &str = "Lanwork";
/// 通知激活器 CLSID。字符串带花括号，和注册表里的写法一致。
pub const ACTIVATOR_CLSID: &str = "{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}";
/// `GUID::from_u128` 用的同一标识。布局与 spike 里已经读回过的 CLSID 相同。
pub const ACTIVATOR_CLSID_U128: u128 = 0xE8E9_EF84_55BA_4A0C_B4DC_9F9F_4A28_068D;
pub const DISPLAY_NAME: &str = "Lanwork";
pub const SHORTCUT_FILE_NAME: &str = "Lanwork.lnk";
pub const RUN_VALUE_NAME: &str = "Lanwork";
/// `System.AppUserModel.*` 的 fmtid。pid 5 是 ID，pid 26 是 ToastActivatorCLSID。
pub const APP_USER_MODEL_FMTID: u128 = 0x9F4C_2855_9F79_4B39_A8D0_E1D4_2DE1_D5F3;
pub const APP_USER_MODEL_ID_PID: u32 = 5;
pub const APP_USER_MODEL_TOAST_PID: u32 = 26;
pub const SLINT_VERSION: &str = "1.18.1";

#[cfg(windows)]
mod link;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 安装目录和便携版压缩包里的文件名。不含 `lanwork-shortcut.exe`。
pub fn program_directory_names() -> Vec<&'static str> {
    let mut names = vec!["lanwork.exe"];
    for file in PROGRAM_DIRECTORY_FILES {
        names.push(file.file_name);
    }
    names.push("Unicode-LICENSE.txt");
    names.push("THIRD_PARTY_NOTICES.txt");
    names
}

pub fn third_party_notices() -> String {
    format!(
        "\
Lanwork {version} 附带的第三方许可。

Everything SDK 1.4 的许可是 Everything-SDK-LICENSE.txt。
Everything SDK 3 的许可是 Everything-SDK3-LICENSE.txt。
可以免费使用、复制、修改、合并、发布、分发、再许可和销售，须保留版权声明和许可声明。

拼音表使用 Unicode 18.0.0 Unihan。许可是 Unicode-LICENSE.txt（Unicode License v3）。

界面使用 Slint {slint}（Royalty-free 2.0）。归属徽标和链接在 GitHub Release 页。

安装向导的简体中文消息是 packaging/ChineseSimplified.isl。
翻译版权与 MIT 许可正文如下，上游是
https://github.com/kira-96/Inno-Setup-Chinese-Simplified-Translation
副本取自 Inno Setup tag is-6_7_3 的 Files/Languages/Unofficial/ChineseSimplified.isl。

{chinese}

编译安装包使用 Inno Setup 6.7.3。编译器不进入安装目录。
下面是 Inno Setup 许可要求的条件，原文来自 tag is-6_7_3 的 license.txt，条件不改。

{inno}
",
        version = env!("CARGO_PKG_VERSION"),
        slint = SLINT_VERSION,
        chinese = include_str!("../../../packaging/ChineseSimplified-LICENSE.txt"),
        inno = include_str!("../../../packaging/Inno-Setup-License.txt"),
    )
}

/// 把程序和许可拷进 `dest`。`exe` 在目标里的名字固定是 `lanwork.exe`。
pub fn stage(exe: &Path, dest: &Path) -> Result<(), String> {
    if !exe.is_file() {
        return Err(format!("找不到程序 {}", exe.display()));
    }
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|err| format!("清空暂存目录失败: {err}"))?;
    }
    fs::create_dir_all(dest).map_err(|err| format!("创建暂存目录失败: {err}"))?;
    copy_file(exe, &dest.join("lanwork.exe"))?;
    let root = repo_root();
    for file in PROGRAM_DIRECTORY_FILES {
        copy_file(&root.join(file.source), &dest.join(file.file_name))?;
    }
    copy_file(
        &root.join("third_party/unihan/LICENSE.txt"),
        &dest.join("Unicode-LICENSE.txt"),
    )?;
    fs::write(dest.join("THIRD_PARTY_NOTICES.txt"), third_party_notices())
        .map_err(|err| format!("写第三方许可说明失败: {err}"))?;
    Ok(())
}

pub fn write_shortcut(exe: &Path, lnk: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        link::write(exe, lnk)
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, lnk);
        Err("lanwork-shortcut 只在 Windows 上写入快捷方式".to_string())
    }
}

pub fn read_shortcut(lnk: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let (aumid, clsid) = link::read(lnk)?;
        let aumid = aumid.unwrap_or_default();
        let clsid = clsid.unwrap_or_default();
        println!("aumid={aumid}");
        println!("clsid={clsid}");
        if aumid != AUMID || !same_clsid(&clsid) {
            return Err(format!(
                "快捷方式属性与安装标识不一致，期望 AUMID {AUMID}、CLSID {ACTIVATOR_CLSID}"
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = lnk;
        Err("lanwork-shortcut 只在 Windows 上读取快捷方式".to_string())
    }
}

pub fn same_clsid(got: &str) -> bool {
    fn norm(text: &str) -> String {
        text.trim()
            .trim_matches(|ch| ch == '{' || ch == '}')
            .to_ascii_uppercase()
    }
    norm(got) == norm(ACTIVATOR_CLSID)
}

fn copy_file(source: &Path, dest: &Path) -> Result<(), String> {
    fs::copy(source, dest).map_err(|err| {
        format!(
            "拷贝 {} 到 {} 失败: {err}",
            source.display(),
            dest.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize_newlines(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                out.push(b'\n');
                index += 2;
            } else {
                out.push(bytes[index]);
                index += 1;
            }
        }
        out
    }

    /// 注释里写「不安装 WebView2」是允许的。这里只拦会把运行时打进安装包的文件名。
    fn installer_bundles_webview(text: &str) -> bool {
        let lower = text.to_ascii_lowercase();
        [
            "microsoftedgewebview2",
            "webview2setup",
            "evergreenbootstrapper",
            "webview2loader.dll",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    #[test]
    fn product_ids_stay_off_the_toast_spike() {
        assert_eq!(AUMID, "Lanwork");
        assert_eq!(DISPLAY_NAME, "Lanwork");
        assert_eq!(RUN_VALUE_NAME, "Lanwork");
        assert_eq!(SHORTCUT_FILE_NAME, "Lanwork.lnk");
        assert_ne!(AUMID, "Lanwork.Spike.Toast");
        assert_eq!(ACTIVATOR_CLSID, "{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}");
        assert_eq!(
            ACTIVATOR_CLSID_U128,
            0xE8E9_EF84_55BA_4A0C_B4DC_9F9F_4A28_068D
        );
        assert_ne!(ACTIVATOR_CLSID, "{DEC1C43B-2AAC-400F-A0B1-C17A05F2B409}");
        assert_eq!(APP_USER_MODEL_ID_PID, 5);
        assert_eq!(APP_USER_MODEL_TOAST_PID, 26);
        assert!(same_clsid(ACTIVATOR_CLSID));
        assert!(same_clsid("e8e9ef84-55ba-4a0c-b4dc-9f9f4a28068d"));
        assert!(!same_clsid("{DEC1C43B-2AAC-400F-A0B1-C17A05F2B409}"));
    }

    #[test]
    fn staged_tree_matches_the_program_directory_list() {
        let temp = std::env::temp_dir().join(format!("lanwork-stage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let exe = temp.join("source-exe.bin");
        fs::create_dir_all(&temp).unwrap();
        fs::write(&exe, b"exe").unwrap();
        let dest = temp.join("out");
        stage(&exe, &dest).unwrap();
        let names = program_directory_names();
        assert!(names.contains(&"Everything64.dll"));
        assert!(names.contains(&"Everything3_x64.dll"));
        assert!(names.contains(&"Everything-SDK-LICENSE.txt"));
        assert!(names.contains(&"Everything-SDK3-LICENSE.txt"));
        assert!(!names.contains(&"lanwork-shortcut.exe"));
        for name in &names {
            let path = dest.join(name);
            assert!(path.is_file(), "missing {}", path.display());
            assert!(fs::metadata(&path).unwrap().len() > 0);
        }
        let sdk = fs::read_to_string(dest.join("Everything-SDK-LICENSE.txt")).unwrap();
        assert!(sdk.contains("Permission is hereby granted"));
        let unicode = fs::read_to_string(dest.join("Unicode-LICENSE.txt")).unwrap();
        assert!(unicode.contains("Unicode"));
        let notices = fs::read_to_string(dest.join("THIRD_PARTY_NOTICES.txt")).unwrap();
        assert!(notices.contains(env!("CARGO_PKG_VERSION")));
        assert!(notices.contains(SLINT_VERSION));
        assert!(notices.contains("Copyright (c) 2019 - 2020 kirakira"));
        assert!(notices.contains("Jordan Russell"));
        assert!(notices.contains("Martijn Laan"));
        assert!(notices.contains("this list of conditions without modification"));
        assert!(!notices.contains("已验收"));
        assert!(!notices.contains("仅供自测"));
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn installer_script_lists_every_staged_file_and_stays_per_user() {
        let iss = include_str!("../../../packaging/lanwork.iss");
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(version, "0.1.0");
        assert!(iss.contains(&format!("#define MyAppVersion \"{version}\"")));
        assert!(iss.contains("AppVersion={#MyAppVersion}"));
        assert!(iss.contains("VersionInfoProductVersion={#MyAppVersion}"));
        assert!(iss.contains("VersionInfoVersion={#MyAppVersion}.0"));
        assert!(iss.contains("OutputBaseFilename=Lanwork-Setup-{#MyAppVersion}"));
        assert!(iss.contains("PrivilegesRequired=lowest"));
        assert!(iss.contains("DefaultDirName={localappdata}\\Programs\\Lanwork"));
        assert!(iss.contains("AppMutex=Local\\Lanwork.SingleInstance.Mutex"));
        assert!(iss.contains("CloseApplications=yes"));
        assert!(iss.contains("CloseApplicationsFilter=lanwork.exe"));
        assert!(iss.contains("RestartApplications=no"));
        assert!(iss.contains("MinVersion=6.1"));
        assert!(!iss.contains("MinVersion=0"));
        assert!(iss.contains("SetupIconFile=..\\crates\\app\\assets\\lanwork.ico"));
        assert!(iss.contains("MessagesFile: \"ChineseSimplified.isl\""));
        assert!(!iss.contains("compiler:Languages\\ChineseSimplified.isl"));
        let messages = include_bytes!("../../../packaging/ChineseSimplified.isl");
        let messages = normalize_newlines(messages);
        assert_eq!(messages.len(), 20905);
        let messages = std::str::from_utf8(&messages).expect("简体中文消息是 UTF-8");
        assert!(
            messages.starts_with("; *** Inno Setup version 6.5.0+ Chinese Simplified messages ***")
        );
        assert!(messages.contains("LanguageName=简体中文"));
        assert!(messages.contains("LanguageID=$0804"));
        assert!(messages.contains("LanguageCodePage=936"));
        assert!(iss.contains("AppUserModelId\\Lanwork"));
        assert!(iss.contains(ACTIVATOR_CLSID));
        assert!(iss.contains("uninsdeletevalue dontcreatekey"));
        assert!(iss.contains("ValueName: \"Lanwork\""));
        assert!(iss.contains("{userprograms}\\Lanwork"));
        assert!(iss.contains("helper\\lanwork-shortcut.exe"));
        assert!(iss.contains("deleteafterinstall"));
        assert!(!iss.contains("[UninstallDelete]"));
        assert!(!iss.contains("{userdocs}"));
        assert!(iss.contains("不安装 WebView2"));
        assert!(!installer_bundles_webview(iss));
        assert!(iss.contains("WizardSilent"));
        assert!(iss.contains("CheckForMutexes"));
        assert!(!iss.contains("PrivilegesRequired=admin"));
        for name in program_directory_names() {
            let needle = format!("Source: \"staging\\{name}\"");
            assert!(iss.contains(&needle), "missing {needle}");
        }
    }

    #[test]
    fn release_notes_show_the_slint_badge_for_a_public_release() {
        let notes = include_str!("../../../packaging/release-notes.md");
        assert!(notes.contains("https://slint.dev"));
        assert!(notes.contains("MadeWithSlint-logo-whitebg.png"));
        assert!(notes.contains("Lanwork-Setup-__VERSION__.exe"));
        assert!(notes.contains("Lanwork-__VERSION__-portable-x64.zip"));
        assert!(notes.contains("__TAG__"));
        assert!(!notes.contains("仅供自测"));
        assert!(!notes.contains("不对外分发"));
        assert!(!notes.contains("已验收"));
        let script = include_str!("../../../packaging/build.ps1");
        assert!(
            script.contains("9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732")
        );
        assert!(script.contains("portable-x64.zip"));
        assert!(script.contains("WaitForExit"));
        assert!(script.contains("packaging\\.tools\\innosetup-$innoVersion"));
        assert!(script.contains("$innoVersion = \"6.7.3\""));
        assert!(script.contains("/PORTABLE=1"));
        assert!(script.contains("verified Inno Setup"));
        assert!(script.contains("/DIR=`\"$innoRoot`\""));
        assert!(!installer_bundles_webview(script));
        let cargo = include_str!("../../../Cargo.toml");
        assert!(cargo.contains(&format!("slint = \"={SLINT_VERSION}\"")));
        let license =
            include_bytes!("../../../third_party/slint/LicenseRef-Slint-Royalty-free-2.0.md");
        let license = normalize_newlines(license);
        assert_eq!(license.len(), 3620);
        assert!(license.starts_with(b"# Slint Royalty-free"));
        let logo = include_bytes!("../../../third_party/slint/MadeWithSlint-logo-whitebg.png");
        assert_eq!(logo.len(), 12351);
        assert_eq!(&logo[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn architecture_records_the_installer_and_drops_the_private_repo_line() {
        let text = include_str!("../../../docs/architecture.md");
        assert!(!text.contains("仓库公开之前不对外分发"));
        assert!(text.contains("Inno Setup 6.7.3"));
        assert!(text.contains("packaging/.tools/innosetup-6.7.3"));
        assert!(text.contains("ChineseSimplified-LICENSE.txt"));
        assert!(text.contains("Inno-Setup-License.txt"));
        assert!(text.contains("Copyright (c) 2019 - 2020 kirakira"));
        assert!(text.contains(ACTIVATOR_CLSID));
        assert!(text.contains("更新未验收"));
        assert!(text.contains("MadeWithSlint-logo-whitebg.png"));
        assert!(text.contains("9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732"));
    }
}
