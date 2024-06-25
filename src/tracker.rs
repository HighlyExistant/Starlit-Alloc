use std::{cell::Cell, collections::{HashMap, LinkedList}};

use nightfall_core::NfPtr;

pub struct AllocationEntry {
    // Has the number of allocation. Ej. If it's the first allocation it will be 0 the second will be 1 etc.
    pub id: u64,
}
// #[cfg(debug_assertions)]
pub struct AllocationTracker {
    entries: Cell<HashMap<usize, AllocationEntry>>,
    pub weight: Cell<u64>,
    count: Cell<u64>,
}

// #[cfg(debug_assertions)]
impl AllocationTracker {
    pub fn new() -> Self {
        Self { entries: Cell::new(HashMap::new()), count: Cell::new(0), weight: Cell::new(0) }
    }
    pub fn insert_entry(&self, offset: usize) {
        unsafe { self.entries.as_ptr().as_mut().unwrap().insert(offset, AllocationEntry { id: self.count.get() }) };
        self.count.set(self.count.get()+1)
    }
    pub fn remove_entry(&self, offset: usize) {
        unsafe { self.entries.as_ptr().as_mut().unwrap().remove_entry(&offset) };
    }
    // pub fn get_entry(&self, offset: &usize) -> &AllocationEntry {
    //     unsafe { self.entries.as_ptr().as_mut().unwrap().get(offset).unwrap() }
    // }
}

// #[repr(transparent)]
// #[cfg(not(debug_assertions))]
// pub struct AllocationTracker;
// #[cfg(not(debug_assertions))]
// impl AllocationTracker {
//     #[inline(always)]
//     pub const fn new() -> Self { Self }
//     #[inline(always)]
//     pub const fn insert_entry(&self, offset: usize, allocation: NfPtr) {}
//     #[inline(always)]
//     pub const fn remove_entry(&self, offset: usize) {}
// }