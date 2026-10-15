#![deny(unsafe_op_in_unsafe_fn)]

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

mod abi;
mod schema;
mod storage;

pub use schema::Schema;
pub use storage::Change;
use storage::Storage;

thread_local! {
    static TRANSACTION: RefCell<Option<Storage>> = const { RefCell::new(None) };
}

pub struct Environment {
    storage: Option<Storage>,
    changes: Vec<Change>,
}

impl Environment {
    pub fn new(schema: Schema) -> Result<Self, String> {
        Ok(Self {
            storage: Some(Storage::new(schema)?),
            changes: Vec::new(),
        })
    }

    // Experimental execution contract: module/prototypes/native-simulation/native-runtime.md.
    pub fn transaction<T>(
        &mut self,
        execute: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        TRANSACTION.with(|active| {
            let mut active = active.borrow_mut();
            if active.is_some() {
                return Err("nested native transactions are not supported".to_owned());
            }
            *active = self.storage.take();
            Ok(())
        })?;
        let result = catch_unwind(AssertUnwindSafe(execute)).unwrap_or_else(|panic| {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("native gameplay panic");
            Err(message.to_owned())
        });
        let mut storage = TRANSACTION
            .with(|active| active.borrow_mut().take())
            .expect("the active transaction owns its storage");
        self.changes.clear();
        if result.is_ok() && storage.fault.is_none() {
            self.changes = storage.commit();
        } else {
            storage.rollback();
        }
        let fault = storage.fault.take();
        self.storage = Some(storage);
        match fault {
            Some(fault) => Err(fault),
            None => result,
        }
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }
}

fn with_storage<T>(operation: impl FnOnce(&mut Storage) -> Result<T, u16>) -> Result<T, u16> {
    TRANSACTION.with(|active| {
        let mut active = active.try_borrow_mut().map_err(|_| 1u16)?;
        let storage = active.as_mut().ok_or(2u16)?;
        catch_unwind(AssertUnwindSafe(|| operation(storage))).unwrap_or_else(|_| {
            storage.fault = Some("native storage operation panicked".into());
            Err(1)
        })
    })
}
