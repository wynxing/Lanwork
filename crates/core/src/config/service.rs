//! 配置的加载和保存。读写走存储层的 `config.json`。
//!
//! 解析失败时存储层隔离原文件，这里改用默认值，不把默认值写回去。
//! 字段值不合法但 JSON 仍能解析时，也不隔离、也不覆盖，内存里用默认值。
//! 不认识的 `schemaVersion` 不隔离，调用方拒绝，本层返回错误。

use std::sync::{Arc, Mutex};

use super::error::ConfigError;
use super::model::{self, Config};
use crate::storage::{self, DocumentId, Error as StoreError, Store};

/// 没有可用的配置文件时，内存里的默认值从哪来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// 文件不存在。
    Missing,
    /// JSON 无法解析，存储层已经隔离原文件。
    Quarantined,
    /// JSON 能解析，但热键或范围不合法。原文件还在。
    Invalid,
}

struct Inner {
    store: Store,
    config: Mutex<Config>,
    fallback: Mutex<Option<Fallback>>,
}

/// 进程内的一份配置。克隆后仍是同一份内存和同一个存储。
#[derive(Clone)]
pub struct ConfigService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ConfigService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigService")
            .field("fallback", &*lock(&self.inner.fallback))
            .finish_non_exhaustive()
    }
}

impl ConfigService {
    pub fn open(store: Store) -> Result<Self, ConfigError> {
        let (config, fallback) = load(&store)?;
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                config: Mutex::new(config),
                fallback: Mutex::new(fallback),
            }),
        })
    }

    #[must_use]
    pub fn store(&self) -> Store {
        self.inner.store.clone()
    }

    #[must_use]
    pub fn current(&self) -> Config {
        lock(&self.inner.config).clone()
    }

    /// 这次启动有没有改用默认值。`None` 表示用的是文件里的配置。
    #[must_use]
    pub fn fallback(&self) -> Option<Fallback> {
        *lock(&self.inner.fallback)
    }

    /// 校验后整份写入。失败时内存和磁盘都保持原样。
    pub fn replace(&self, next: Config) -> Result<Config, ConfigError> {
        let next = model::normalize(next)?;
        self.inner
            .store
            .write_json(&DocumentId::Config, &next)
            .map_err(ConfigError::from)?;
        *lock(&self.inner.config) = next.clone();
        *lock(&self.inner.fallback) = None;
        Ok(next)
    }
}

fn load(store: &Store) -> Result<(Config, Option<Fallback>), ConfigError> {
    // 先按任意 JSON 读。语法错误由存储层隔离。能解析但字段不合法时保留原文件。
    let value = match store.read_json::<serde_json::Value>(&DocumentId::Config) {
        Ok(None) => return Ok((Config::default(), Some(Fallback::Missing))),
        Ok(Some(value)) => value,
        Err(StoreError::Quarantined { .. }) => {
            return Ok((Config::default(), Some(Fallback::Quarantined)));
        }
        Err(err) => return Err(err.into()),
    };
    let config = match serde_json::from_value::<Config>(value) {
        Ok(config) => config,
        Err(_) => return Ok((Config::default(), Some(Fallback::Invalid))),
    };
    match model::normalize(config) {
        Ok(config) => Ok((config, None)),
        Err(ConfigError::UnsupportedSchema { found }) => {
            Err(ConfigError::UnsupportedSchema { found })
        }
        Err(_) => Ok((Config::default(), Some(Fallback::Invalid))),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    storage::lock_mutex(mutex)
}
