use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

#[derive(Debug)]
pub(super) struct CompileCache {
    capacity: usize,
    entries: Mutex<Entries>,
}

#[derive(Debug, Default)]
struct Entries {
    functions: HashMap<Vec<u8>, Box<[u8]>>,
    order: VecDeque<Vec<u8>>,
    bytes: usize,
}

impl CompileCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Mutex::default(),
        }
    }
}

impl wasmtime::CacheStore for CompileCache {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let entries = self.entries.lock().ok()?;
        entries
            .functions
            .get(key)
            .map(|value| Cow::Owned(value.to_vec()))
    }

    fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
        let size = key.len().saturating_mul(2).saturating_add(value.len());
        if size > self.capacity {
            return false;
        }
        let Ok(mut entries) = self.entries.lock() else {
            return false;
        };
        if entries.functions.contains_key(key) {
            return true;
        }
        while entries.bytes > self.capacity - size {
            let Some(oldest) = entries.order.pop_front() else {
                return false;
            };
            if let Some(value) = entries.functions.remove(&oldest) {
                entries.bytes -= oldest.len() * 2 + value.len();
            }
        }
        entries.bytes += size;
        entries.order.push_back(key.to_vec());
        entries
            .functions
            .insert(key.to_vec(), value.into_boxed_slice());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::CacheStore;

    #[test]
    fn eviction_preserves_newer_functions_within_the_byte_limit() {
        let cache = CompileCache::new(10);
        assert!(cache.insert(b"a", vec![1; 3]));
        assert!(cache.insert(b"b", vec![2; 3]));
        assert_eq!(cache.get(b"a").as_deref(), Some(&[1; 3][..]));
        assert!(cache.insert(b"c", vec![3; 3]));
        assert!(cache.get(b"a").is_none());
        assert_eq!(cache.get(b"b").as_deref(), Some(&[2; 3][..]));
        assert_eq!(cache.get(b"c").as_deref(), Some(&[3; 3][..]));
        assert_eq!(cache.entries.lock().unwrap().bytes, 10);
    }

    #[test]
    fn oversized_functions_do_not_evict_cached_functions() {
        let cache = CompileCache::new(5);
        assert!(cache.insert(b"a", vec![1; 3]));
        assert!(!cache.insert(b"b", vec![2; 4]));
        assert_eq!(cache.get(b"a").as_deref(), Some(&[1; 3][..]));
        assert!(cache.insert(b"a", vec![9; 3]));
        assert_eq!(cache.entries.lock().unwrap().bytes, 5);
    }

    #[test]
    fn cached_code_preserves_results_fuel_and_epoch_interrupts() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Debug)]
        struct CountedCache {
            store: CompileCache,
            inserts: AtomicUsize,
        }
        impl CacheStore for CountedCache {
            fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
                self.store.get(key)
            }
            fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
                self.inserts.fetch_add(1, Ordering::Relaxed);
                self.store.insert(key, value)
            }
        }
        let cache = std::sync::Arc::new(CountedCache {
            store: CompileCache::new(1024 * 1024),
            inserts: AtomicUsize::new(0),
        });
        let mut config = wasmtime::Config::new();
        config
            .cranelift_opt_level(wasmtime::OptLevel::None)
            .consume_fuel(true)
            .epoch_interruption(true);
        config
            .enable_incremental_compilation(cache.clone())
            .unwrap();
        let engine = wasmtime::Engine::new(&config).unwrap();
        let constant = b"\0asm\x01\0\0\0\x01\x05\x01\x60\0\x01\x7f\x03\x02\x01\0\x07\x07\x01\x03run\0\0\x0a\x06\x01\x04\0\x41\x2a\x0b";
        let mut fuel = Vec::new();
        let mut inserts = Vec::new();
        for _ in 0..2 {
            let module = wasmtime::Module::new(&engine, constant).unwrap();
            let mut store = wasmtime::Store::new(&engine, ());
            store.set_fuel(100).unwrap();
            store.set_epoch_deadline(1);
            let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
            assert_eq!(
                instance
                    .get_typed_func::<(), i32>(&mut store, "run")
                    .unwrap()
                    .call(&mut store, ())
                    .unwrap(),
                42
            );
            fuel.push(store.get_fuel().unwrap());
            inserts.push(cache.inserts.load(Ordering::Relaxed));
        }
        assert_eq!(fuel[0], fuel[1]);
        assert!(inserts[0] > 0);
        assert_eq!(inserts[0], inserts[1]);
        assert!(fuel[0] < 100);
        let looping = b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x02\x01\0\x07\x07\x01\x03run\0\0\x0a\x09\x01\x07\0\x03\x40\x0c\0\x0b\x0b";
        inserts.clear();
        for _ in 0..2 {
            let module = wasmtime::Module::new(&engine, looping).unwrap();
            inserts.push(cache.inserts.load(Ordering::Relaxed));
            let mut store = wasmtime::Store::new(&engine, ());
            store.set_fuel(u64::MAX).unwrap();
            store.set_epoch_deadline(1);
            let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
            let ticker = engine.clone();
            let tick = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                ticker.increment_epoch();
            });
            let error = instance
                .get_typed_func::<(), ()>(&mut store, "run")
                .unwrap()
                .call(&mut store, ())
                .unwrap_err();
            tick.join().unwrap();
            assert_eq!(
                error.downcast_ref::<wasmtime::Trap>(),
                Some(&wasmtime::Trap::Interrupt)
            );
        }
        assert_eq!(inserts[0], inserts[1]);
    }
}
