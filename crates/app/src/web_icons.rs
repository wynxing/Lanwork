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
    ///
    /// 返回的地址已经记成进行中。调用方要直接取，不能再交给 [`claim`]：
    /// 那次认领会看见进行中就丢掉，请求不会启动，也没有人把它从进行中拿掉。
    pub(crate) fn claim_again(&self, url: &str) -> Option<String> {
        let key = icon_cache_key(url)?;
        let mut state = lock(&self.state);
        state.failed.remove(&key);
        if !state.inflight.insert(key) {
            return None;
        }
        Some(url.to_owned())
    }

    #[cfg(test)]
    fn is_inflight(&self, url: &str) -> bool {
        icon_cache_key(url).is_some_and(|key| lock(&self.state).inflight.contains(&key))
    }

    #[cfg(test)]
    fn is_failed(&self, url: &str) -> bool {
        icon_cache_key(url).is_some_and(|key| lock(&self.state).failed.contains(&key))
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

/// 打开这条后再取一次。已经在取的直接返回。
///
/// 只取 [`take_open_retry`] 返回的地址，不再 [`WebIcons::claim`]。
/// 线程没起来时，这个地址记成这次失败。
pub(crate) fn retry(icons: Arc<WebIcons>, url: &str) -> Result<(), String> {
    let Some(url) = take_open_retry(&icons, url) else {
        return Ok(());
    };
    fetch(icons, vec![url])
}

/// 打开重试要取的那一条。已经记成进行中。空表示正在取，或地址不能做缓存键。
fn take_open_retry(icons: &WebIcons, url: &str) -> Option<String> {
    icons.claim_again(url)
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
        finish_attempt(attempt, &cache, &mut source, &mut decoder, true);
    }
}

fn finish_attempt(
    attempt: &mut Attempt,
    cache: &WebIconCache,
    source: &mut impl IconSource,
    decoder: &mut impl IconDecoder,
    notify: bool,
) {
    let Some(image) = load_web_icon(&attempt.url, cache, source, decoder) else {
        return;
    };
    if notify {
        let url = attempt.url.clone();
        let _ = slint::invoke_from_event_loop(move || deliver(vec![(url, image)]));
    }
    attempt.ok = true;
}

fn deliver(icons: Vec<(String, RgbaImage)>) {
    let again = icons.clone();
    crate::shelves_page::receive_web_icons(icons);
    crate::shelf_float::receive_web_icons(again);
}

fn lock(mutex: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{WebIcons, finish_attempt, take_open_retry};
    use lanwork_core::dispatch::RgbaImage;
    use lanwork_core::shelves::{Fetched, IconDecoder, IconSource};
    use std::sync::Arc;

    struct Counting {
        gets: Vec<String>,
        succeed: Option<&'static str>,
    }

    impl IconSource for Counting {
        fn get(&mut self, url: &str, _limit: usize) -> Option<Fetched> {
            self.gets.push(url.to_owned());
            let succeed = self.succeed?;
            if !url.contains(succeed) {
                return None;
            }
            Some(Fetched {
                final_url: url.to_owned(),
                content_type: Some("image/png".into()),
                body: b"\x89PNG".to_vec(),
            })
        }
    }

    struct RedPixel;

    impl IconDecoder for RedPixel {
        fn decode(&mut self, bytes: &[u8], _content_type: Option<&str>) -> Option<RgbaImage> {
            if bytes.is_empty() {
                return None;
            }
            RgbaImage::new(1, 1, vec![255, 0, 0, 255])
        }
    }

    /// 列表失败之后，打开再试必须真正取一次并在结束时结算。
    /// `claim` 会把进行中的地址丢掉，所以这里走和 `retry` 相同的 `claim_again` 结果。
    fn open_retry(
        icons: &Arc<WebIcons>,
        url: &str,
        source: &mut Counting,
        decoder: &mut RedPixel,
    ) -> Option<bool> {
        let claimed = take_open_retry(icons, url)?;
        assert!(
            icons.claim(std::slice::from_ref(&claimed)).is_empty(),
            "进行中的地址再 claim 会被丢掉"
        );
        assert!(icons.claim_again(url).is_none(), "正在取时不重复排");
        let mut attempt = super::Attempt {
            icons: Arc::clone(icons),
            url: claimed,
            ok: false,
        };
        finish_attempt(&mut attempt, &icons.cache, source, decoder, false);
        let ok = attempt.ok;
        drop(attempt);
        Some(ok)
    }

    fn list_fail(icons: &Arc<WebIcons>, url: &str, source: &mut Counting, decoder: &mut RedPixel) {
        let pending = icons.claim(std::slice::from_ref(&url.to_owned()));
        assert_eq!(pending, vec![url.to_owned()]);
        let mut attempt = super::Attempt {
            icons: Arc::clone(icons),
            url: pending[0].clone(),
            ok: false,
        };
        finish_attempt(&mut attempt, &icons.cache, source, decoder, false);
        assert!(!attempt.ok);
        drop(attempt);
        assert!(icons.is_failed(url));
        assert!(!icons.is_inflight(url));
        assert!(icons.claim(&[url.to_owned()]).is_empty());
    }

    #[test]
    fn failed_open_retries_then_succeeds_or_fails_again() {
        let root =
            std::env::temp_dir().join(format!("lanwork-web-icons-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let icons = Arc::new(WebIcons::new(&root));
        let ok_url = "https://example.com/ok";
        let bad_url = "https://example.com/bad";
        let mut source = Counting {
            gets: Vec::new(),
            succeed: None,
        };
        let mut decoder = RedPixel;

        list_fail(&icons, ok_url, &mut source, &mut decoder);
        let before = source.gets.len();
        source.succeed = Some("example.com/ok");
        assert_eq!(
            open_retry(&icons, ok_url, &mut source, &mut decoder),
            Some(true)
        );
        assert!(source.gets.len() > before);
        assert!(
            source.gets[before..]
                .iter()
                .any(|url| url.contains("example.com/ok"))
        );
        assert!(!icons.is_failed(ok_url));
        assert!(!icons.is_inflight(ok_url));

        list_fail(&icons, bad_url, &mut source, &mut decoder);
        let before = source.gets.len();
        assert_eq!(
            open_retry(&icons, bad_url, &mut source, &mut decoder),
            Some(false)
        );
        assert!(source.gets.len() > before);
        assert!(icons.is_failed(bad_url));
        assert!(!icons.is_inflight(bad_url));
        assert!(icons.claim(&[bad_url.to_owned()]).is_empty());
        assert!(icons.claim_again(bad_url).is_some());
        assert!(icons.is_inflight(bad_url));
        icons.settle(bad_url, false);
        assert!(!icons.is_inflight(bad_url));
        assert!(icons.is_failed(bad_url));

        let _ = std::fs::remove_dir_all(&root);
    }
}
