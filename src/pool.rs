use std::{alloc::Layout, collections::HashMap, ops::Mul, os::raw::c_void, ptr::NonNull, result, sync::Arc};
use ash::vk::Handle;
use nightfall_core::{buffers::{Buffer, BufferCreateInfo, MappedMemory}, device::LogicalDevice, error::NightfallError, memory::DevicePointer, NfPtr};

use crate::HostDeviceConversions;

struct PoolAllocatorInner {
    free_chunks: Vec<usize>,
    alloc_layout: Layout,
    // contains the total number of allocated objects
    allocations: usize,
    // contains the size in bytes of the buffer.
    size: usize,
}

impl PoolAllocatorInner {
    pub fn new(layout: Layout, capacity: usize) -> Self {
        Self { free_chunks: vec![0], alloc_layout: layout, allocations: 0, size: layout.size()*capacity }
    }
    // the offset will be in terms of bytes not on the layout size.
    pub fn free_inner(&mut self, offset: usize) {
        self.allocations -= 1;
        self.free_chunks.push(offset);
    }
    pub fn allocate_inner(&mut self) -> Option<usize> {
        if self.free_chunks.is_empty() {
            None
        } else if self.free_chunks.len() == 1 { 
            // if the length of free chunks equals 1 it is garunteed to be at the very end of the allocations so instead of
            // popping the allocation we can write to the offset.

            // if the free_chunks length equals 1 and it's pointing to the final element then there are no allocations at the end and must be popped
            self.allocations += 1;
            if self.free_chunks[0] == (self.size-self.alloc_layout.size()) {
                self.free_chunks.pop()
            } else {
                let offset = self.free_chunks[0];
                self.free_chunks[0] += self.alloc_layout.size();
                Some(offset)
            }
        } else {
            self.free_chunks.pop()
        }
    }
}

pub struct PoolAllocator<T> {
    inner: PoolAllocatorInner,
    buffer: Arc<Buffer>,
    map: Option<MappedMemory<T>>,
}
impl<T> TryFrom<Arc<Buffer>> for PoolAllocator<T> {
    type Error = NightfallError;
    fn try_from(value: Arc<Buffer>) -> Result<Self, Self::Error> {
        let size = value.size();
        let range = 0..value.size();
        let map = if value.is_mappable() {
            Some(value.clone().map_memory::<T>(range).unwrap())
        } else {
            None
        };
        let alloc_size = std::mem::size_of::<T>();
        let inner = PoolAllocatorInner::new(Layout::from_size_align(
            alloc_size, std::mem::align_of::<T>()).unwrap(), 
            size/alloc_size
        );
        Ok(Self { 
            inner, 
            map, 
            buffer: value.clone() 
        })
    }
}
impl<T> PoolAllocator<T> {
    pub fn new(device: Arc<LogicalDevice>, info: BufferCreateInfo) -> Result<Self, NightfallError> {
        let buffer = Arc::new(Buffer::new(
            device, 
            info
        ).unwrap());
        Self::try_from(buffer)
    }
    pub fn allocate(&mut self) -> Result<NfPtr, NightfallError> {
        self.inner.allocate_inner()
        .ok_or(NightfallError::OutOfMemory(
            std::mem::size_of::<T>(), 
            self.inner.size-self.inner.allocations.mul(std::mem::size_of::<T>()
        ))).map(|alloc|{
            let ptr = if let Some(ptr) = self.buffer.get_address() {
                Some(ptr+alloc)
            } else {
                None
            };
            NfPtr::new(self.buffer.handle().as_raw(), alloc, ptr, self.inner.alloc_layout.size())
        })
    }
    pub fn deallocate(&mut self, allocation: NfPtr) -> Result<(), NightfallError> {
        self.inner.free_inner(allocation.offset() as usize);
        Ok(())
    }
    // only works if device addressing is enabled
    pub fn as_device_ptr(&self, allocation: NfPtr) -> Option<DevicePointer> {
        if let Some(addr) = self.buffer.get_address() {
            Some(DevicePointer::from_raw((addr.addr() + allocation.offset()) as u64))
        } else {
            None
        }
    }
    pub fn allocated(&self) -> usize {
        self.inner.allocations*self.inner.alloc_layout.size()
    }
    pub fn size(&self) -> usize {
        self.inner.size
    }
    pub fn is_host_mappable(&self) -> Option<std::ptr::NonNull<std::os::raw::c_void>> {
        let ptr = self.map.as_ref()?.ptr() as *mut c_void;
        Some(NonNull::new(ptr).unwrap())
    }
}

impl<T> HostDeviceConversions for PoolAllocator<T> {
    fn as_host_ptr(&self, allocation: NfPtr) -> Option<*const c_void> {
        if let Some(host) = &self.map {
            Some(unsafe { host.mut_ptr().add(allocation.offset() as usize) as *const c_void })
        } else {
            None
        }
    }
    fn as_host_mut_ptr(&self, allocation: NfPtr) -> Option<*mut c_void> {
        if let Some(host) = &self.map {
            Some(unsafe { host.mut_ptr().add(allocation.offset() as usize) as *mut c_void })
        } else {
            None
        }
    }
    fn as_device_ptr(&self, allocation: NfPtr) -> Option<DevicePointer> {
        if let Some(addr) = self.buffer.get_address() {
            Some(DevicePointer::from_raw((addr.addr() + allocation.offset())  as u64))
        } else {
            None
        }
    }
}