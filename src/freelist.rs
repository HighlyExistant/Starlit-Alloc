use std::{alloc::Layout, cell::Cell, collections::{HashMap, HashSet}, ptr::NonNull, sync::Arc};

use ash::vk::Handle;
use nightfall_core::{buffers::{Buffer, BufferCreateInfo, BufferUsageFlags, MappedMemory, MemoryPropertyFlags}, device::LogicalDevice, memory::DevicePointer, NfPtr};

use crate::{error::StarlitAllocError, tracker::AllocationTracker, GeneralAllocator, HostDeviceConversions};

#[repr(u8)]
#[derive(PartialEq, Copy, Clone, Debug)]
enum AllocationStatus {
    Free,
    Linear,
}

#[derive(Clone, Copy, Debug)]
struct FreeListHeader {
    offset: usize,
    size: usize,
    prev: Option<usize>,
    next: Option<usize>,
    status: AllocationStatus,
}
#[inline]
const fn align_down(val: usize, alignment: usize) -> usize { val & !(alignment - 1usize) }
#[inline]
const fn align_up(val: usize, alignment: usize) -> usize { align_down(val + alignment - 1usize, alignment) }

pub struct FreeListAllocatorInternal {
    pub(crate) granularity: usize,
    pub(crate) size: usize,
    pub(crate) allocated: usize,
    // uses pointers for the HashSet
    pub(crate) free_chunks: HashSet<usize>,
    /// Key: Pointer
    /// Value: Header
    chunks: HashMap<usize, FreeListHeader>,
    tracker: AllocationTracker,
}
impl FreeListAllocatorInternal {
    pub fn new(size: usize, granularity: usize) -> Self {
        let mut chunks = HashMap::<usize, FreeListHeader>::new();
        let offset = 0;
        chunks.insert(0, FreeListHeader { 
            offset, 
            size, 
            prev: None, 
            next: None ,
            status: AllocationStatus::Free, 
        });
        let mut free_chunks = HashSet::<usize>::new();
        free_chunks.insert(offset);
        Self { granularity, size, allocated: 0, free_chunks, chunks, tracker: AllocationTracker::new() }
    }
    pub fn allocate_inner(&mut self, layout: Layout) -> Result<(usize, usize), StarlitAllocError> {
        self.tracker.weight.set(self.tracker.weight.get()+1);
        println!("ALLOCATED {}", self.tracker.weight.get());
        if layout.size() == 0 {
            return Err(StarlitAllocError::ZeroSize);
        }
        let mut best_fit = Some((0usize, 0usize));
        let aligned_size = self.aligned_size(layout);
        let free_size = self.size - self.allocated;
        if aligned_size > free_size {
            return Err(StarlitAllocError::OutOfMemory {
                size: aligned_size,
                free_size
            });
        }
        for free_chunk in self.free_chunks.iter() {
            let chunk = self.chunks.get(free_chunk)
                .ok_or(StarlitAllocError::InternalError("Free chunk does not exist".into()))?;
            // if the chunk size is larger than the aligned size
            // the allocation will not fit and should be skipped
            if chunk.size < aligned_size {
                continue;
            }
            if let Some((offset, size)) = &mut best_fit {
                *offset = chunk.offset;
                *size = chunk.size;
            } else {
                best_fit = Some((chunk.offset, chunk.size));
            }
        }
        let (best_offset, best_size) = best_fit.ok_or(StarlitAllocError::InternalError(
            "Failed to find best fit".into()
        ))?;
        // if the chunk size is larger than the allocated size it means that it has extra
        // memory in the header that can be turned into another free chunk
        let next_offset = if best_size > aligned_size {
            // Load new free chunk from this allocation
            let new_free_chunk = FreeListHeader {
                offset: best_offset + aligned_size, // get the end of the allocation
                prev: Some(best_offset), // the previous chunk is the chunk being currently allocated
                next: None, // this field is added after an allocation is made onto the chunk
                size: best_size - aligned_size, // the new size is adjusted for the current allocation
                status: AllocationStatus::Free, // this will serve as the next free chunk
            };
            self.chunks.insert(new_free_chunk.offset, new_free_chunk);
            self.free_chunks.insert(new_free_chunk.offset);
            
            Some(new_free_chunk.offset)
        } else {
            // since we check whether or not the allocated chunks memory size is larger than or equal to
            // the aligned size we are trying to allocate, we already know that our allocation will fit,
            // so if we reach this part it means that it perfectly fits into our aligned size.
            None
        };
        // remove the currently allocated chunk from the free list
        self.free_chunks.remove(&best_offset);
        // adjust the allocated chunk to have the correct size.
        let allocated_chunk = self.chunks.get_mut(&best_offset)
            .ok_or(StarlitAllocError::InternalError("Free chunk does not exist".into()))?;
        allocated_chunk.size = aligned_size;
        allocated_chunk.status = AllocationStatus::Linear;
        allocated_chunk.next = next_offset;
        self.allocated += aligned_size;
        self.tracker.insert_entry(best_offset);
        Ok((best_offset,aligned_size))
    }
    pub fn merge_free_chunks(&mut self, offset_left: usize, offset_right: usize) -> Result<(), StarlitAllocError> {
        // the right chunk of the merge will be removed to be merged
        let (right_size, right_next) = {
            let chunk = self.chunks.remove(&offset_right).ok_or(StarlitAllocError::InternalError(
                "The chunk of memory is not available to be merged".into()
            ))?;
            self.free_chunks.remove(&chunk.offset);

            (chunk.size, chunk.next)
        };
        // Merge into left chunk
        {
            let chunk = self.chunks.get_mut(&offset_left).ok_or(StarlitAllocError::InternalError(
                "The chunk of memory is not available to be merged".into()
            ))?;
            chunk.next = right_next;
            chunk.size += right_size;
        }
        // Patch pointers
        if let Some(right_next) = right_next {
            let chunk = self.chunks.get_mut(&right_next).ok_or(StarlitAllocError::InternalError(
                "The chunk of memory is not available to be merged".into()
            ))?;
            chunk.prev = Some(offset_left);
        }
        Ok(())
    }
    pub fn free_inner(&mut self, offset: usize) -> Result<(), StarlitAllocError> {
        println!("DEALLOCATED {}", self.tracker.weight.get());
        let (next_id, prev_id) = {
            let chunk = self.chunks.get_mut(&offset).ok_or(
                StarlitAllocError::InvalidFree
            )?;
            chunk.status = AllocationStatus::Free;

            self.allocated = self.allocated.checked_sub(chunk.size).ok_or(StarlitAllocError::InvalidFree)?;

            self.free_chunks.insert(chunk.offset);
            self.tracker.remove_entry(chunk.offset);

            (chunk.next, chunk.prev)
        };
        
        if let Some(next_id) = next_id {
            if self.chunks[&next_id].status == AllocationStatus::Free {
                self.merge_free_chunks(offset, next_id)?;
            }
        }
        if let Some(prev_id) = prev_id {
            if self.chunks[&prev_id].status == AllocationStatus::Free {
                self.merge_free_chunks(prev_id, offset)?;
            }
        }
        self.tracker.weight.set(self.tracker.weight.get()-1);
        Ok(())
    }
    pub fn aligned_size(&self, layout: Layout) -> usize {
        align_up(layout.size(), self.granularity)
    }
    fn remaining_size(&self) -> usize {
        self.size - self.allocated
    }
    fn allocatable(&self, layout: Layout) -> bool {
        let aligned_size = self.aligned_size(layout);
        let free_size = self.size - self.allocated;
        aligned_size < free_size
        
    }
}
impl Drop for FreeListAllocatorInternal {
    fn drop(&mut self) {
        // if cfg!(debug_assertions) {
            let allocated_chunks = self.chunks.iter()
                .filter_map(|(offset, chunk)|{
                if let Some(offset) = self.free_chunks.iter().find(|free| free.eq(&offset)) {
                    Some((*offset, *chunk))
                } else {
                    None
                }
            }).collect::<Vec<(usize, FreeListHeader)>>();
            // the main block counts as an allocation, but we don't count it as a memory leak
            if allocated_chunks.len() == 1 { return; }
            let mut iter = allocated_chunks.iter();
            // iter.next().unwrap(); // first iter is simply the free chunk for the next allocation
            for (offset, header) in iter {
                println!("memory leak at offset {} of size {}", offset, header.size);
            }
        // }
    }
}
pub struct FreeListArena {
    freelist: Cell<FreeListAllocatorInternal>,
    buffer: Arc<Buffer>,
    map: Option<MappedMemory<u8>>,
}


impl TryFrom<Arc<Buffer>> for FreeListArena {
    type Error = StarlitAllocError;
    fn try_from(value: Arc<Buffer>) -> Result<Self, Self::Error> {
        let size = value.size();
        let range = 0..value.size();
        let map = if value.is_mappable() {
            Some(value.clone().map_memory::<u8>(range).unwrap())
        } else {
            None
        };
        let freelist = Cell::new(FreeListAllocatorInternal::new(size, value.alignment()));
        Ok(Self { 
            freelist, 
            map, 
            buffer: value.clone() 
        })
    }
}
impl FreeListArena {
    pub fn new(device: Arc<LogicalDevice>, info: BufferCreateInfo) -> Result<Self, StarlitAllocError> {
        let buffer = Arc::new(Buffer::new(
            device, 
            info
        ).unwrap());
        Self::try_from(buffer)
    }
    fn freelist(&self) -> &FreeListAllocatorInternal {
        unsafe { self.freelist.as_ptr().as_mut().unwrap() }
    }
    pub fn allocate_object<T>(&self) -> Result<NfPtr, StarlitAllocError> {
        self.allocate(Layout::from_size_align(std::mem::size_of::<T>(), std::mem::align_of::<T>()).unwrap())
    }
    pub fn device(&self) -> Arc<LogicalDevice> {
        self.buffer.device()
    }
    pub fn remaining_size(&self) -> usize {
        self.freelist().remaining_size()
    }
    fn allocatable(&self, layout: Layout) -> bool {
        self.freelist().allocatable(layout)
    }
    pub fn aligned_size(&self, layout: Layout) -> usize {
        self.freelist().aligned_size(layout)
    }
}

impl GeneralAllocator for FreeListArena {
    fn allocate(&self, layout: Layout) -> Result<NfPtr, StarlitAllocError> {
        let freelist = unsafe { self.freelist.as_ptr().as_mut().unwrap() };
        let (offset, size) = freelist.allocate_inner(layout)?;
        let ptr = if let Some(ptr) = self.buffer.get_address() {
            Some(ptr+offset)
        } else {
            None
        };
        Ok(NfPtr::new(self.buffer.handle().as_raw(), offset, ptr, size))
    }
    fn allocated(&self) -> usize {
        let freelist = unsafe { self.freelist.as_ptr().as_mut().unwrap() };
        freelist.allocated
    }
    fn size(&self) -> usize {
        let freelist = unsafe { self.freelist.as_ptr().as_mut().unwrap() };
        freelist.size
    }
    fn deallocate(&self, allocation: NfPtr, layout: Layout) -> Result<(), StarlitAllocError> {
        let freelist = unsafe { self.freelist.as_ptr().as_mut().unwrap() };
        freelist.free_inner(allocation.offset() as usize)
    }
    fn is_pointer_mappable(&self, allocation: NfPtr) -> Option<NonNull<std::os::raw::c_void>> {
        let ptr = self.map.as_ref()?.ptr() as *mut std::os::raw::c_void;
        Some(NonNull::new(ptr)?)
    }
    fn is_host_mappable(&self) -> bool {
        self.buffer.is_mappable()
    }
}
impl HostDeviceConversions for FreeListArena {
    fn as_host_ptr(&self, allocation: NfPtr) -> Option<*const std::os::raw::c_void> {
        if let Some(host) = &self.map {
            Some(unsafe { host.mut_ptr().add(allocation.offset() as usize) as *const std::os::raw::c_void })
        } else {
            None
        }
    }
    fn as_host_mut_ptr(&self, allocation: NfPtr) -> Option<*mut std::os::raw::c_void> {
        if let Some(host) = &self.map {
            Some(unsafe { host.mut_ptr().add(allocation.offset() as usize) as *mut std::os::raw::c_void })
        } else {
            None
        }
    }
    fn as_device_ptr(&self, allocation: NfPtr) -> Option<DevicePointer> {
        if let Some(addr) = self.buffer.get_address() {
            Some(DevicePointer::from_raw(addr.addr() as u64 + allocation.offset() as u64))
        } else {
            None
        }
    }
}
/// A Free List Allocator for Gpu Memory
pub struct FreeListAllocator {
    device: Arc<LogicalDevice>,
    chunks: Cell<Vec<FreeListArena>>,
    is_mappable: bool,
}

impl TryFrom<Arc<Buffer>> for FreeListAllocator {
    type Error = StarlitAllocError;
    fn try_from(value: Arc<Buffer>) -> Result<Self, Self::Error> {
        let freelist = FreeListArena::try_from(value.clone())?;
        Ok(Self { 
            device: value.device(),
            is_mappable: value.is_mappable(),
            chunks: Cell::new(vec![freelist])
        })
    }
}
impl FreeListAllocator {
    /// Creates a [`FreeListAllocator`] the same way you would create a Buffer
    /// 
    /// # Examples
    /// 
    /// ```
    /// // creating LogicalDevice and Queues
    /// use nightfall_core::{
    ///     buffers::{BufferCreateInfo, BufferUsageFlags, MemoryPropertyFlags},
    ///     device::LogicalDeviceBuilder, 
    ///     instance::InstanceBuilder, 
    ///     queue::DeviceQueueCreateFlags, 
    /// };
    /// use starlit_alloc::FreeListAllocator;
    /// let instance = InstanceBuilder::new().build().unwrap();
    /// let physical_device = instance.enumerate_physical_devices().unwrap().next().unwrap();
    /// // doesn't matter for this test
    /// let queue_family_index = 0;
    /// let (device, mut queues) = LogicalDeviceBuilder::new()
    ///     .add_queue(DeviceQueueCreateFlags::empty(), queue_family_index as u32, 1, 0, &1.0)
    ///     .build(physical_device.clone()).unwrap();
    /// 
    /// let freelist = FreeListAllocator::new(device.clone(), BufferCreateInfo {
    ///     buffer_addressing: true,
    ///     usage: BufferUsageFlags::STORAGE_BUFFER,
    ///     size: 64000000, // 64 mb
    ///     properties: MemoryPropertyFlags::HOST_VISIBLE|MemoryPropertyFlags::HOST_COHERENT,
    ///     ..Default::default()
    /// }).unwrap();
    /// ```
    pub fn new(device: Arc<LogicalDevice>, info: BufferCreateInfo) -> Result<Self, StarlitAllocError> {
        let buffer = Arc::new(Buffer::new(
            device, 
            info
        ).unwrap());
        Self::try_from(buffer)
    }
    pub fn get_pointer_chunk(&self, allocation: NfPtr) -> Option<&FreeListArena>{
        self.chunks().iter().find(|c|c.buffer.handle() == allocation.buffer())
    }
    fn chunks(&self) -> &mut Vec<FreeListArena> {
        let x = unsafe { self.chunks.as_ptr().as_mut().unwrap() };
        x
    }
    fn get_allocatable_chunk(&self, layout: Layout) -> Option<&FreeListArena> {
        let chunks = self.chunks();
        if let Some(freelist) = chunks.iter().find(|c|c.allocatable(layout)) {
            Some(freelist)
        } else {
            let buffer = chunks[0].buffer.clone();
            let info = BufferCreateInfo {
                buffer_addressing: buffer.buffer_addressing_enabled(),
                size: buffer.size(),
                usage: buffer.usage(),
                properties: buffer.properties(),
                ..Default::default()
            };
            let chunk = FreeListArena::new(self.device.clone(), info).ok()?;
            unsafe {
                self.chunks().push(chunk);
                chunks.last()
            }
        }
    }
}
impl HostDeviceConversions for FreeListAllocator {
    fn as_host_ptr(&self, allocation: NfPtr) -> Option<*const std::os::raw::c_void> {
        let chunk = self.get_pointer_chunk(allocation)?;
        chunk.as_host_ptr(allocation)
    }
    fn as_host_mut_ptr(&self, allocation: NfPtr) -> Option<*mut std::os::raw::c_void> {
        let chunk = self.get_pointer_chunk(allocation)?;
        chunk.as_host_mut_ptr(allocation)
    }
    fn as_device_ptr(&self, allocation: NfPtr) -> Option<DevicePointer> {
        let chunk = self.get_pointer_chunk(allocation)?;
        chunk.as_device_ptr(allocation)
    }
}
impl GeneralAllocator for FreeListAllocator {
    fn allocate(&self, layout: Layout) -> Result<NfPtr, StarlitAllocError> {
        let chunk = self.get_allocatable_chunk(layout).unwrap();
        chunk.allocate(layout)
    }
    fn allocated(&self) -> usize {
        self.chunks().iter().fold(0, |a, b|{ a + b.allocated() })
    }
    fn size(&self) -> usize {
        self.chunks().iter().fold(0, |a, b|{ a + b.size() })
    }
    fn deallocate(&self, allocation: NfPtr, layout: Layout) -> Result<(), StarlitAllocError> {
        let freelist = unsafe { self.get_pointer_chunk(allocation).unwrap().freelist.as_ptr().as_mut().unwrap() };
        freelist.free_inner(allocation.offset() as usize)
    }
    fn reallocate(&self, allocation: NfPtr, old_layout: Layout, new_layout: Layout) -> Result<NfPtr, StarlitAllocError> {
        let chunk = self.get_pointer_chunk(allocation).unwrap();
        if chunk.allocatable(new_layout) {
            chunk.reallocate(allocation, old_layout, new_layout)
        } else {
            let seperate_chunk = self.get_allocatable_chunk(new_layout).unwrap();
            if let Some(ptr) = seperate_chunk.is_pointer_mappable(allocation) {
                debug_assert!(
                    new_layout.size() >= old_layout.size(),
                    "`new_layout.size()` must be greater than or equal to `old_layout.size()`"
                );
                let new_allocation = seperate_chunk.allocate(new_layout)?;
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
    }
    fn is_pointer_mappable(&self, allocation: NfPtr) -> Option<NonNull<std::os::raw::c_void>> {
        let ptr = self.get_pointer_chunk(allocation)?;
        ptr.is_pointer_mappable(allocation)
    }
    fn is_host_mappable(&self) -> bool {
        let chunk = unsafe { self.chunks.as_ptr().as_mut().unwrap().get(0).unwrap() };
        chunk.buffer.is_mappable()
    }
}


#[cfg(test)]
mod tests {
    use nightfall_core::{buffers::{BufferCreateInfo, BufferUsageFlags, MemoryPropertyFlags}, device::{LogicalDevice, LogicalDeviceBuilder}, instance::InstanceBuilder, queue::{DeviceQueueCreateFlags, Queue, QueueFlags}, Version};

    use crate::util::standard;

    use super::*;
    
    #[test]
    fn test_freelist_standard_invalid_free_error() {
        let (device, mut queues) = standard();

        // FreeList Creation Test
        let freelist = FreeListAllocator::new(device.clone(), BufferCreateInfo {
            buffer_addressing: true,
            usage: BufferUsageFlags::STORAGE_BUFFER,
            size: 64000000, // 64 mb
            properties: MemoryPropertyFlags::HOST_VISIBLE|MemoryPropertyFlags::HOST_COHERENT,
            ..Default::default()
        }).unwrap();
        let alloc = freelist.allocate(Layout::new::<u32>()).unwrap();
        freelist.deallocate(alloc, Layout::new::<u32>());
        
        match freelist.deallocate(alloc, Layout::new::<u32>()) {
            Err(e) => {
                assert!(e == StarlitAllocError::InvalidFree, "Recieved another error that was not Invalid Free, probably an internal issue with allocator");
            }
            _ => { panic!("Didn't Fail to deallocate meaning there was an invalid free") }
        }
    }
}
