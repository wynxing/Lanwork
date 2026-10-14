//! 收纳页和收纳悬浮窗共用的网站图标任务。
//!
//! 保存网页引用的线程不进这里。失败只记下这一次运行，不改分组文件。

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use lanwork_core::dispatch::RgbaImage;
use lanwork_core::shelves::{IconDecoder, IconSource, WebIconCache, icon_cache_key, load_web_icon};

use crate::favicon_http;
use crate::favicon_wic::{self, ComGuard};

struct State {
    inflight: HashSet<String>,
    failed: HashSet<String>,
}

static SHARED: OnceLock<Arc<WebIcons>> = OnceLock::new();

/// 启动时调用一次。收纳页和收纳悬浮窗共用这一份进行中记录和失败记录。
pub(crate) fn install(data_dir: &Path) {
    let _ = SHARED.set(Arc::new(WebIcons::new(data_dir)));
}

pub(crate) fn shared() -> Arc<WebIcons> {
    Arc::clone(SHARED.get().expect("网站图标缓存还没有安装"))
}

pub(crate) struct WebIcons {
    cache: WebIconCache,
    state: Mutex<State>,
}

impl WebIcons {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            cache: WebIconCache::new(data_dir),
            state: Mutex::new(State {
                inflight: HashSet::new(),
                failed: HashSet::new(),
            }),
        }
    }

    /// 还没在取、这次运行也没失败过的地址。调用后记成正在取。
    pub(crate) fn claim(&self, urls: &[String]) -> Vec<String> {
        let mut state = lock(&self.state);
        let mut pending = Vec::new();
        for url in urls {
            let Some(key) = icon_cache_key(url) else {
                continue;
            };
            if state.inflight.contains(&key) || state.failed.contains(&key) {
                continue;
            }
            state.inflight.insert(key);
            pending.push(url.clone());
        }
        pending
    }

    /// 打开这条时再试一次。正在取的不重复排。
    pub(crate) fn claim_again(&self, url: &str) -> Option<String> {
        let key = icon_cache_key(url)?;
        let mut state = lock(&self.state);
        state.failed.remove(&key);
        if !state.inflight.insert(key) {
            return None;
        }
        Some(url.to_owned())
    }

    fn settle(&self, url: &str, ok: bool) {
        let Some(key) = icon_cache_key(url) else {
            return;
        };
        let mut state = lock(&self.state);
        state.inflight.remove(&key);
        if !ok {
            state.failed.insert(key);
        }
    }
}

struct Attempt {
    icons: Arc<WebIcons>,
    url: String,
    ok: bool,
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.icons.settle(&self.url, self.ok);
    }
}

struct HttpGet;

impl IconSource for HttpGet {
    fn get(&mut self, url: &str, limit: usize) -> Option<lanwork_core::shelves::Fetched> {
        favicon_http::get(url, limit)
    }
}

struct WicDecode;

impl IconDecoder for WicDecode {
    fn decode(&mut self, bytes: &[u8], content_type: Option<&str>) -> Option<RgbaImage> {
        favicon_wic::decode(bytes, content_type)
    }
}

/// 后台取这些地址。线程没起来时，这些地址记成这次失败，调用方照常返回。
pub(crate) fn fetch(icons: Arc<WebIcons>, urls: Vec<String>) -> Result<(), String> {
    let cache = icons.cache.clone();
    let attempts = urls
        .into_iter()
        .map(|url| Attempt {
            icons: Arc::clone(&icons),
            url,
            ok: false,
        })
        .collect::<Vec<_>>();
    std::thread::Builder::new()
        .name("lanwork-favicon".into())
        .spawn(move || worker(cache, attempts))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn worker(cache: WebIconCache, mut attempts: Vec<Attempt>) {
    let _com = ComGuard::enter();
    let mut source = HttpGet;
    let mut decoder = WicDecode;
    for attempt in &mut attempts {
        let Some(image) = load_web_icon(&attempt.url, &cache, &mut source, &mut decoder) else {
            continue;
        };
        let url = attempt.url.clone();
        let _ = slint::invoke_from_event_loop(move || deliver(vec![(url, image)]));
        attempt.ok = true;
    }
}

fn deliver(icons: Vec<(String, RgbaImage)>) {
    let again = icons.clone();
    crate::shelves_page::receive_web_icons(icons);
    crate::shelf_float::receive_web_icons(again);
}

fn lock(mutex: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
