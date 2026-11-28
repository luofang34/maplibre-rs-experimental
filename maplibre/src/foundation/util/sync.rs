//! Synchronization lock helpers that avoid `Atomics.wait` on `wasm32 + atomics`.

/// Acquires a [`std::sync::Mutex`] without calling `memory.atomic.wait32` on `wasm32 + atomics`
/// (where the browser main thread forbids `Atomics.wait`).
pub fn lock_unpoisoned<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        loop {
            match mutex.try_lock() {
                Ok(guard) => return guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => core::hint::spin_loop(),
            }
        }
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Acquires a read guard on [`std::sync::RwLock`] without `Atomics.wait` on `wasm32 + atomics`.
pub(crate) fn read_unpoisoned<T>(lock: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        loop {
            match lock.try_read() {
                Ok(guard) => return guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => core::hint::spin_loop(),
            }
        }
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        lock.read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Acquires a write guard on [`std::sync::RwLock`] without `Atomics.wait` on `wasm32 + atomics`.
pub(crate) fn write_unpoisoned<T>(
    lock: &std::sync::RwLock<T>,
) -> std::sync::RwLockWriteGuard<'_, T> {
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    {
        loop {
            match lock.try_write() {
                Ok(guard) => return guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => core::hint::spin_loop(),
            }
        }
    }
    #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
    {
        lock.write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
