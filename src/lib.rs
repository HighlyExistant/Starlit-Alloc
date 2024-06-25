#![allow(unused)]
use std::{alloc::Layout, ptr::NonNull, sync::Arc};

use nightfall_core::{buffers::{Buffer, MappedMemory}, NfPtr};

mod freelist;
mod stack;
mod pool;
mod error;
mod allocator;
mod tracker;
pub mod util;
pub use freelist::*;
pub use stack::*;
pub use pool::*;
pub use allocator::*;
pub use error::*;
/// The trait [`HostDeviceConversions`] allows a common interface for all allocators to convert between different types
/// of representations on an [`Allocation`] that belongs to the same chunk of an allocator. 
pub trait HostDeviceConversions {
    fn as_host_ptr(&self, allocation: NfPtr) -> Option<*const std::os::raw::c_void>;
    fn as_host_mut_ptr(&self, allocation: NfPtr) -> Option<*mut std::os::raw::c_void>;
    fn as_device_ptr(&self, allocation: NfPtr) -> Option<nightfall_core::memory::DevicePointer>;
}
/// Allocates Memory on the Gpu
/// 
/// # Examples
/// 
/// ```
/// use std::alloc::Layout;
/// use nightfall_core::{
///     buffers::{BufferCreateInfo, BufferUsageFlags, MemoryPropertyFlags},
///     device::LogicalDeviceBuilder, 
///     instance::InstanceBuilder, 
///     queue::DeviceQueueCreateFlags, 
/// };
/// use starlit_alloc::{FreeListAllocator, GeneralAllocator, HostDeviceConversions};
/// let instance = InstanceBuilder::new().build().unwrap();
/// let physical_device = instance.enumerate_physical_devices().unwrap().next().unwrap();
/// // doesn't matter for this test
/// let queue_family_index = 0;
/// let (device, _) = LogicalDeviceBuilder::new()
///     .add_queue(DeviceQueueCreateFlags::empty(), queue_family_index as u32, 1, 0, &1.0)
///     .build(physical_device.clone()).unwrap();
/// let freelist = FreeListAllocator::new(device.clone(), BufferCreateInfo {
///     buffer_addressing: true,
///     usage: BufferUsageFlags::STORAGE_BUFFER,
///     size: 64000000, // 64 mb
///     properties: MemoryPropertyFlags::HOST_VISIBLE|MemoryPropertyFlags::HOST_COHERENT,
///     ..Default::default()
/// }).unwrap();
/// let layout = Layout::new::<i32>();
/// let allocation = freelist.allocate(layout).unwrap();
/// let host = freelist.as_host_mut_ptr(allocation).unwrap().cast::<i32>();
/// unsafe {
///     *host = 10;
///     assert!(*host == 10);
/// }
/// let new_layout = Layout::new::<[i32; 8]>();
/// // reallocations only work on host visible allocators
/// freelist.reallocate(allocation, layout, new_layout);
/// let host = freelist.as_host_mut_ptr(allocation).unwrap().cast::<[i32; 8]>();
/// 
/// unsafe {
///     // even after being reallocated, the previously stored values will still be present.
///     assert!((*host)[0] == 10);
///     for i in 1..8 {
///         (*host)[i] = i as i32;
///         assert!((*host)[i] == i as i32);
///     }
/// }
/// // All allocations must be deallocated otherwise a memory leak will occur.
/// freelist.deallocate(allocation, layout).unwrap();
/// ```
pub trait GeneralAllocator: HostDeviceConversions {
    fn allocate(&self, layout: Layout) -> Result<NfPtr, StarlitAllocError>;
    fn deallocate(&self, allocation: NfPtr, layout: Layout) -> Result<(), StarlitAllocError>;
    fn reallocate(&self, allocation: NfPtr, old_layout: Layout, new_layout: Layout) -> Result<NfPtr, StarlitAllocError> {
        if let Some(ptr) = self.is_pointer_mappable(allocation) {
            debug_assert!(
                new_layout.size() >= old_layout.size(),
                "`new_layout.size()` must be greater than or equal to `old_layout.size()`"
            );
            let new_allocation = self.allocate(new_layout)?;
            let old_offset = ptr.as_ptr() as usize + allocation.offset();
            let new_offset = ptr.as_ptr() as usize + new_allocation.offset();
            unsafe {
                std::ptr::copy_nonoverlapping(old_offset as *const std::os::raw::c_void, new_offset as *mut std::os::raw::c_void, old_layout.size());
                self.deallocate(allocation, old_layout)?;
            };
            Ok(new_allocation)
        } else {
            Err(StarlitAllocError::MemoryWriteError)
        }
    }
    /// returns the total amount of bytes within the allocators arena
    fn size(&self) -> usize;
    /// returns the total amount of bytes allocated
    fn allocated(&self) -> usize;
    /// returns the total amount of bytes remaining in the allocators arena
    fn remaining(&self) -> usize { self.size() - self.allocated() }
    /// returns a host mapped pointer to the start of memory.
    fn is_pointer_mappable(&self, allocation: NfPtr) -> Option<NonNull<std::os::raw::c_void>> { None }
    /// returns a host mapped pointer to the start of memory.
    fn is_host_mappable(&self) -> bool { false }
}
