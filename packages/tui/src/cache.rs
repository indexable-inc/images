use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::SystemTime;
use uuid::Uuid;

use crate::{Error, types::TuiInstance};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct CacheValue {
    pub uuid: Uuid,
    pub instance: TuiInstance,
    pub created: SystemTime,
    pub reference_count: i16,
}

#[derive(Default)]
pub struct Cache {
    cache: Mutex<HashMap<Uuid, CacheValue>>,
}

impl Cache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, uuid: Uuid, instance: TuiInstance) -> Result<()> {
        let mut lock = self.cache.lock();
        let cache_value = CacheValue {
            uuid,
            instance,
            created: SystemTime::now(),
            reference_count: 1,
        };
        lock.insert(uuid, cache_value);
        Ok(())
    }

    pub fn get(&self, uuid: &Uuid, increment: bool) -> Result<Option<CacheValue>> {
        let mut lock = self.cache.lock();
        let Some(cv) = lock.get_mut(uuid) else {
            return Ok(None);
        };

        let cv_clone = cv.clone();
        if increment {
            cv.reference_count += 1;
        }
        Ok(Some(cv_clone))
    }

    pub fn remove(&self, key: &Uuid, force: bool) -> Result<Option<CacheValue>> {
        let mut lock = self.cache.lock();

        let reference_count = lock.get_mut(key).map(|cache_value| {
            cache_value.reference_count -= 1;
            cache_value.reference_count
        });

        let removed = if force || reference_count.unwrap_or_default() < 1 {
            lock.remove(key)
        } else {
            None
        };

        Ok(removed)
    }

    pub fn list(&self) -> Vec<CacheValue> {
        self.cache.lock().values().cloned().collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }
}
