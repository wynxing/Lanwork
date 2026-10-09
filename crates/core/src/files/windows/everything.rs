//! 动态加载随包的 Everything SDK。先 1.5（SDK3），再 1.4。
//!
//! 1.5 先连未命名实例，管道不存在再连 `1.5a`。不为查询启动进程。

use std::ffi::c_void;
use std::mem::{self, size_of};
use std::path::Path;

use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::s;

use super::wide::{NAME_BUF, pcwstr, string_from_wide, wide_null, wide_path};
use crate::files::backend::{ProbeNote, ProbeNoteKind, SourceFailure};
use crate::files::model::{FileHit, FileKind, MAX_FILE_RESULTS};
use crate::files::service::probe_failed_line;

const EVERYTHING_OK: u32 = 0;
const EVERYTHING_ERROR_IPC: u32 = 2;
const EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND: u32 = 0xE000_0002;
const ALPHA_INSTANCE: &str = "1.5a";
const PROPERTY_NAME: u32 = 0;

pub(crate) enum LoadFail {
    Missing,
    Failed(String),
}

pub(crate) struct SdkView {
    pub status: SdkStatus,
    pub note: Option<ProbeNote>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SdkStatus {
    Ready,
    NotReady,
    NotRunning,
}

pub(crate) struct Sdk3 {
    module: HMODULE,
    connect: ConnectW,
    destroy: DestroyClient,
    last_error: GetDword,
    is_db_loaded: IsDb3,
    major: Ver3,
    minor: Ver3,
    revision: Ver3,
    build: Ver3,
    create_state: CreateState,
    destroy_state: DestroyState,
    set_text: SetText,
    set_viewport: SetSize,
    add_property: AddProperty,
    set_match_path: SetStateBool,
    search: SearchFn,
    destroy_results: DestroyResults,
    count: CountFn,
    name: Name3,
    full_path: Name3,
    is_folder: IsFolder3,
    client: *mut c_void,
    version: String,
}

type ConnectW = unsafe extern "system" fn(*const u16) -> *mut c_void;
type DestroyClient = unsafe extern "system" fn(*mut c_void) -> i32;
type GetDword = unsafe extern "system" fn() -> u32;
type IsDb3 = unsafe extern "system" fn(*mut c_void) -> i32;
type Ver3 = unsafe extern "system" fn(*mut c_void) -> u32;
type CreateState = unsafe extern "system" fn() -> *mut c_void;
type DestroyState = unsafe extern "system" fn(*mut c_void) -> i32;
type SetText = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;
type SetSize = unsafe extern "system" fn(*mut c_void, usize) -> i32;
type AddProperty = unsafe extern "system" fn(*mut c_void, u32) -> i32;
type SetStateBool = unsafe extern "system" fn(*mut c_void, i32) -> i32;
type SearchFn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> *mut c_void;
type DestroyResults = unsafe extern "system" fn(*mut c_void) -> i32;
type CountFn = unsafe extern "system" fn(*mut c_void) -> usize;
type Name3 = unsafe extern "system" fn(*mut c_void, usize, *mut u16, usize) -> usize;
type IsFolder3 = unsafe extern "system" fn(*mut c_void, usize) -> i32;

// SAFETY: 模块句柄和客户端指针只在文件来源的锁里使用。FreeLibrary 与 DestroyClient 在 Drop 里各跑一次。
unsafe impl Send for Sdk3 {}

impl Sdk3 {
    pub(crate) fn load(path: &Path) -> Result<Self, LoadFail> {
        let module = load_module(path)?;
        match Self::bind(module) {
            Ok(sdk) => Ok(sdk),
            Err(message) => {
                // SAFETY: bind 失败，没有其他持有者。
                unsafe {
                    let _ = FreeLibrary(module);
                }
                Err(LoadFail::Failed(message))
            }
        }
    }

    fn bind(module: HMODULE) -> Result<Self, String> {
        Ok(Self {
            module,
            connect: sym(module, s!("Everything3_ConnectW"))?,
            destroy: sym(module, s!("Everything3_DestroyClient"))?,
            last_error: sym(module, s!("Everything3_GetLastError"))?,
            is_db_loaded: sym(module, s!("Everything3_IsDBLoaded"))?,
            major: sym(module, s!("Everything3_GetMajorVersion"))?,
            minor: sym(module, s!("Everything3_GetMinorVersion"))?,
            revision: sym(module, s!("Everything3_GetRevision"))?,
            build: sym(module, s!("Everything3_GetBuildNumber"))?,
            create_state: sym(module, s!("Everything3_CreateSearchState"))?,
            destroy_state: sym(module, s!("Everything3_DestroySearchState"))?,
            set_text: sym(module, s!("Everything3_SetSearchTextW"))?,
            set_viewport: sym(module, s!("Everything3_SetSearchViewportCount"))?,
            add_property: sym(module, s!("Everything3_AddSearchPropertyRequest"))?,
            set_match_path: sym(module, s!("Everything3_SetSearchMatchPath"))?,
            search: sym(module, s!("Everything3_Search"))?,
            destroy_results: sym(module, s!("Everything3_DestroyResultList"))?,
            count: sym(module, s!("Everything3_GetResultListCount"))?,
            name: sym(module, s!("Everything3_GetResultNameW"))?,
            full_path: sym(module, s!("Everything3_GetResultFullPathNameW"))?,
            is_folder: sym(module, s!("Everything3_IsFolderResult"))?,
            client: std::ptr::null_mut(),
            version: "未知".to_owned(),
        })
    }

    pub(crate) fn probe(&mut self) -> SdkView {
        if !self.client.is_null() {
            match self.classify_open_client() {
                OpenClient::Ready => {
                    return SdkView {
                        status: SdkStatus::Ready,
                        note: None,
                    };
                }
                OpenClient::NotReady => {
                    return SdkView {
                        status: SdkStatus::NotReady,
                        note: None,
                    };
                }
                OpenClient::Dead => self.drop_client(),
            }
        }
        let unnamed = self.connect_instance(None);
        if unnamed.connected {
            return self.view_after_connect();
        }
        let alpha = self.connect_instance(Some(ALPHA_INSTANCE));
        if alpha.connected {
            return self.view_after_connect();
        }
        let unexpected = [unnamed.error, alpha.error]
            .into_iter()
            .find(|err| *err != EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND);
        if let Some(err) = unexpected {
            return SdkView {
                status: SdkStatus::NotRunning,
                note: Some(failure_note(
                    "sdk3",
                    "未知",
                    &format!("last_error={err:#X}"),
                )),
            };
        }
        SdkView {
            status: SdkStatus::NotRunning,
            note: None,
        }
    }

    pub(crate) fn query(&mut self, text: &str) -> Result<Vec<FileHit>, SourceFailure> {
        if self.client.is_null() {
            return Err(SourceFailure::new("Everything 1.5 未连接"));
        }
        let version = self.version.clone();
        let wide = wide_null(text);
        // SAFETY: client 已连接。搜索状态若创建成功，由 guard 销毁。失败时立刻读 last error。
        let (state, create_err) = unsafe {
            let state = (self.create_state)();
            let err = if state.is_null() {
                (self.last_error)()
            } else {
                0
            };
            (state, err)
        };
        if state.is_null() {
            let err = create_err;
            return Err(SourceFailure::new(format!(
                "CreateSearchState 失败，版本={version} last_error={err:#X}"
            )));
        }
        let mut guard = SearchGuard {
            destroy_state: self.destroy_state,
            destroy_results: self.destroy_results,
            state,
            results: std::ptr::null_mut(),
        };
        // SAFETY: state 由 guard 持有。搜索串以 0 结尾。viewport 不超过 50。不请求 NAME 时 GetResultNameW 得到空串，所以请求属性 0。
        let (results, err) = unsafe {
            (self.set_text)(guard.state, wide.as_ptr());
            (self.set_match_path)(guard.state, 0);
            (self.set_viewport)(guard.state, MAX_FILE_RESULTS);
            (self.add_property)(guard.state, PROPERTY_NAME);
            let results = (self.search)(self.client, guard.state);
            let err = (self.last_error)();
            (results, err)
        };
        if results.is_null() {
            drop(guard);
            self.drop_client();
            return Err(SourceFailure::new(format!(
                "Everything3_Search 失败，版本={version} last_error={err:#X}"
            )));
        }
        guard.results = results;
        // SAFETY: results 刚由 Everything3_Search 返回，且非空。
        let count = unsafe { (self.count)(results) }.min(MAX_FILE_RESULTS);
        let mut hits = Vec::with_capacity(count);
        for index in 0..count {
            let name = read_sdk3(self.name, results, index);
            let path = read_sdk3(self.full_path, results, index);
            // SAFETY: results 仍由 guard 持有，index 小于结果数。
            let folder = unsafe { (self.is_folder)(results, index) } != 0;
            if let Some(hit) = hit_from_parts(name, path, folder) {
                hits.push(hit);
            }
        }
        drop(guard);
        Ok(hits)
    }

    fn view_after_connect(&mut self) -> SdkView {
        match self.classify_open_client() {
            OpenClient::Ready => SdkView {
                status: SdkStatus::Ready,
                note: None,
            },
            OpenClient::NotReady => SdkView {
                status: SdkStatus::NotReady,
                note: None,
            },
            OpenClient::Dead => {
                self.drop_client();
                SdkView {
                    status: SdkStatus::NotRunning,
                    note: None,
                }
            }
        }
    }

    fn classify_open_client(&mut self) -> OpenClient {
        // SAFETY: client 非空。先读数据库，再读这次的 last error。0 不改写成管道不存在。
        let (loaded, err) = unsafe {
            let loaded = (self.is_db_loaded)(self.client) != 0;
            let err = (self.last_error)();
            (loaded, err)
        };
        if loaded {
            self.version = read_version(
                self.major,
                self.minor,
                self.revision,
                self.build,
                self.client,
            );
            OpenClient::Ready
        } else if err == EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND {
            OpenClient::Dead
        } else {
            OpenClient::NotReady
        }
    }

    fn connect_instance(&mut self, instance: Option<&str>) -> ConnectOutcome {
        self.drop_client();
        let wide;
        let ptr = if let Some(name) = instance {
            wide = wide_null(name);
            wide.as_ptr()
        } else {
            std::ptr::null()
        };
        // SAFETY: 实例名要么是空指针，要么以 0 结尾。失败后立刻读 GetLastError，包含 0。
        let (client, err) = unsafe {
            let client = (self.connect)(ptr);
            let err = (self.last_error)();
            (client, err)
        };
        if client.is_null() {
            ConnectOutcome {
                connected: false,
                error: err,
            }
        } else {
            self.client = client;
            self.version = read_version(self.major, self.minor, self.revision, self.build, client);
            ConnectOutcome {
                connected: true,
                error: err,
            }
        }
    }

    fn drop_client(&mut self) {
        if self.client.is_null() {
            return;
        }
        // SAFETY: client 来自 ConnectW，只释放一次。
        unsafe {
            (self.destroy)(self.client);
        }
        self.client = std::ptr::null_mut();
        self.version = "未知".to_owned();
    }
}

impl Drop for Sdk3 {
    fn drop(&mut self) {
        self.drop_client();
        // SAFETY: 这个模块只由本结构持有。
        unsafe {
            let _ = FreeLibrary(self.module);
        }
    }
}

enum OpenClient {
    Ready,
    NotReady,
    Dead,
}

struct ConnectOutcome {
    connected: bool,
    error: u32,
}

fn read_version(
    major_fn: Ver3,
    minor_fn: Ver3,
    revision_fn: Ver3,
    build_fn: Ver3,
    client: *mut c_void,
) -> String {
    // SAFETY: client 非空。四个函数只读服务端版本。
    let (major, minor, revision, build) = unsafe {
        (
            major_fn(client),
            minor_fn(client),
            revision_fn(client),
            build_fn(client),
        )
    };
    if major == 0 && minor == 0 && revision == 0 && build == 0 {
        "未知".to_owned()
    } else {
        format!("{major}.{minor}.{revision}.{build}")
    }
}

struct SearchGuard {
    destroy_state: DestroyState,
    destroy_results: DestroyResults,
    state: *mut c_void,
    results: *mut c_void,
}

impl Drop for SearchGuard {
    fn drop(&mut self) {
        if !self.results.is_null() {
            // SAFETY: results 来自 Everything3_Search，只销毁一次。
            unsafe {
                (self.destroy_results)(self.results);
            }
        }
        if !self.state.is_null() {
            // SAFETY: state 来自 CreateSearchState，只销毁一次。
            unsafe {
                (self.destroy_state)(self.state);
            }
        }
    }
}

fn read_sdk3(read: Name3, results: *mut c_void, index: usize) -> String {
    let mut buf = vec![0u16; NAME_BUF];
    // SAFETY: results 在 SearchGuard 丢掉之前有效。缓冲区长度传给 SDK。
    unsafe {
        read(results, index, buf.as_mut_ptr(), buf.len());
    }
    string_from_wide(&buf)
}

pub(crate) struct Sdk14 {
    module: HMODULE,
    set_search: SetSearchW,
    set_match_path: SetBool,
    set_max: SetDword,
    query: QueryW,
    last_error: GetDword,
    is_db_loaded: GetBool,
    num_results: GetDword,
    is_folder: IsFolder14,
    full_path: FullPath14,
    file_name: Name14,
    major: GetDword,
    minor: GetDword,
    revision: GetDword,
    build: GetDword,
}

type SetSearchW = unsafe extern "system" fn(*const u16);
type SetBool = unsafe extern "system" fn(i32);
type SetDword = unsafe extern "system" fn(u32);
type QueryW = unsafe extern "system" fn(i32) -> i32;
type GetBool = unsafe extern "system" fn() -> i32;
type IsFolder14 = unsafe extern "system" fn(u32) -> i32;
type FullPath14 = unsafe extern "system" fn(u32, *mut u16, u32) -> u32;
type Name14 = unsafe extern "system" fn(u32) -> *const u16;

// SAFETY: 1.4 的函数是进程内全局状态。只在文件来源的锁里调用，模块在 Drop 里释放。
unsafe impl Send for Sdk14 {}

impl Sdk14 {
    pub(crate) fn load(path: &Path) -> Result<Self, LoadFail> {
        let module = load_module(path)?;
        match Self::bind(module) {
            Ok(sdk) => Ok(sdk),
            Err(message) => {
                // SAFETY: bind 失败，没有其他持有者。
                unsafe {
                    let _ = FreeLibrary(module);
                }
                Err(LoadFail::Failed(message))
            }
        }
    }

    fn bind(module: HMODULE) -> Result<Self, String> {
        Ok(Self {
            module,
            set_search: sym(module, s!("Everything_SetSearchW"))?,
            set_match_path: sym(module, s!("Everything_SetMatchPath"))?,
            set_max: sym(module, s!("Everything_SetMax"))?,
            query: sym(module, s!("Everything_QueryW"))?,
            last_error: sym(module, s!("Everything_GetLastError"))?,
            is_db_loaded: sym(module, s!("Everything_IsDBLoaded"))?,
            num_results: sym(module, s!("Everything_GetNumResults"))?,
            is_folder: sym(module, s!("Everything_IsFolderResult"))?,
            full_path: sym(module, s!("Everything_GetResultFullPathNameW"))?,
            file_name: sym(module, s!("Everything_GetResultFileNameW"))?,
            major: sym(module, s!("Everything_GetMajorVersion"))?,
            minor: sym(module, s!("Everything_GetMinorVersion"))?,
            revision: sym(module, s!("Everything_GetRevision"))?,
            build: sym(module, s!("Everything_GetBuildNumber"))?,
        })
    }

    pub(crate) fn probe(&self) -> SdkView {
        let (loaded, err) = self.db_status();
        if loaded {
            SdkView {
                status: SdkStatus::Ready,
                note: None,
            }
        } else if err == EVERYTHING_ERROR_IPC {
            SdkView {
                status: SdkStatus::NotRunning,
                note: None,
            }
        } else if err == EVERYTHING_OK {
            SdkView {
                status: SdkStatus::NotReady,
                note: None,
            }
        } else {
            SdkView {
                status: SdkStatus::NotRunning,
                note: Some(failure_note(
                    "sdk14",
                    &self.version_or_unknown(),
                    &format!("last_error={err:#X}"),
                )),
            }
        }
    }

    pub(crate) fn query(&self, text: &str) -> Result<Vec<FileHit>, SourceFailure> {
        let wide = wide_null(text);
        let version = self.version_or_unknown();
        // SAFETY: 搜索串以 0 结尾。SetMax 为 50。QueryW 的 1 表示等到结果返回。只匹配文件名。
        let (ok, err) = unsafe {
            (self.set_search)(wide.as_ptr());
            (self.set_match_path)(0);
            (self.set_max)(u32::try_from(MAX_FILE_RESULTS).unwrap_or(50));
            let ok = (self.query)(1) != 0;
            let err = (self.last_error)();
            (ok, err)
        };
        if !ok {
            return Err(SourceFailure::new(format!(
                "Everything 1.4 查询失败，版本={version} last_error={err:#X}"
            )));
        }
        // SAFETY: QueryW 已成功。结果数量是这次查询的可见条数。
        let returned = unsafe { (self.num_results)() };
        let take = usize::try_from(returned).unwrap_or(0).min(MAX_FILE_RESULTS);
        let mut hits = Vec::with_capacity(take);
        for index in 0..take {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            let mut path_buf = vec![0u16; NAME_BUF];
            // SAFETY: index 小于本次结果数。文件名在下一次 SDK 调用前拷走。路径缓冲区在调用期间有效。
            let (name, folder) = unsafe {
                let name = wide_from_ptr((self.file_name)(index));
                let _ = (self.full_path)(
                    index,
                    path_buf.as_mut_ptr(),
                    u32::try_from(path_buf.len()).unwrap_or(0),
                );
                let folder = (self.is_folder)(index) != 0;
                (name, folder)
            };
            let name = string_from_wide(&name);
            if let Some(hit) = hit_from_parts(name, string_from_wide(&path_buf), folder) {
                hits.push(hit);
            }
        }
        Ok(hits)
    }

    fn db_status(&self) -> (bool, u32) {
        // SAFETY: DLL 已加载。调用后立刻读 last error。
        let (loaded, err) = unsafe {
            let loaded = (self.is_db_loaded)() != 0;
            let err = (self.last_error)();
            (loaded, err)
        };
        (loaded, err)
    }

    fn version_or_unknown(&self) -> String {
        // SAFETY: DLL 已加载。未连接时这些函数返回 0，调用方不把 0 改写成 IPC 错误。
        let (major, minor, revision, build) = unsafe {
            (
                (self.major)(),
                (self.minor)(),
                (self.revision)(),
                (self.build)(),
            )
        };
        if major == 0 && minor == 0 && revision == 0 && build == 0 {
            "未知".to_owned()
        } else {
            format!("{major}.{minor}.{revision}.{build}")
        }
    }
}

impl Drop for Sdk14 {
    fn drop(&mut self) {
        // SAFETY: 这个模块只由本结构持有。
        unsafe {
            let _ = FreeLibrary(self.module);
        }
    }
}

fn failure_note(sdk: &str, version: &str, detail: &str) -> ProbeNote {
    ProbeNote {
        kind: ProbeNoteKind::ProbeFailed,
        line: probe_failed_line(sdk, version, detail),
    }
}

fn hit_from_parts(name: String, path: String, folder: bool) -> Option<FileHit> {
    let name = if name.is_empty() {
        path.rsplit(['\\', '/']).next().unwrap_or("").to_owned()
    } else {
        name
    };
    if name.is_empty() && path.is_empty() {
        return None;
    }
    Some(FileHit {
        name,
        path,
        kind: if folder {
            FileKind::Folder
        } else {
            FileKind::File
        },
    })
}

fn wide_from_ptr(ptr: *const u16) -> Vec<u16> {
    if ptr.is_null() {
        return Vec::new();
    }
    let mut len = 0usize;
    // SAFETY: SDK 返回的文件名以 0 结尾，读取不超过 NAME_BUF。指针只在这次取值里使用。
    unsafe {
        while len < NAME_BUF && *ptr.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(ptr, len).to_vec()
    }
}

fn load_module(path: &Path) -> Result<HMODULE, LoadFail> {
    if !path.is_file() {
        return Err(LoadFail::Missing);
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let wide = wide_path(&canon);
    // SAFETY: 路径以 0 结尾，缓冲区在调用期间有效。
    unsafe { LoadLibraryW(pcwstr(&wide)) }.map_err(|err| {
        LoadFail::Failed(format!(
            "LoadLibraryW 失败，hresult=0x{:08X}",
            err.code().0 as u32
        ))
    })
}

fn sym<T>(module: HMODULE, name: windows::core::PCSTR) -> Result<T, String> {
    // SAFETY: 模块句柄仍然有效。名字指针由 s! 生成，只在这里读成字符串。
    let proc = unsafe { GetProcAddress(module, name) };
    let proc = proc.ok_or_else(|| {
        // SAFETY: 同上，GetProcAddress 失败时仍可读这个常量名字。
        let label = unsafe { name.to_string() }.unwrap_or_default();
        format!("DLL 里没有 {label}")
    })?;
    if size_of::<T>() != size_of_val(&proc) {
        return Err("函数指针大小不一致".to_owned());
    }
    // SAFETY: T 是同等大小的系统调用约定函数指针，只从非空的 GetProcAddress 结果拷贝。
    Ok(unsafe { mem::transmute_copy(&proc) })
}
