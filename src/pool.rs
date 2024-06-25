use std::{alloc::Layout, cell::Cell, collections::HashMap, ops::Mul, os::raw::c_void, ptr::NonNull, result, sync::Arc};
use ash::vk::Handle;
use nightfall_core::{buffers::{Buffer, BufferCreateFlags, BufferCreateInfo, BufferUsageFlags, MappedMemory, MemoryPropertyFlags}, device::LogicalDevice, error::NightfallError, memory::DevicePointer, NfPtr, NfPtrType};

use crate::HostDeviceConversions;

struct PoolArenaInner {
    free_chunks: Vec<usize>,
    alloc_layout: Layout,
    // contains the total number of allocated objects
    allocations: usize,
    // contains the size in bytes of the buffer.
    size: usize,
}

impl PoolArenaInner {
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

pub struct PoolArena<T> {
    inner: Cell<PoolArenaInner>,
    buffer: Arc<Buffer>,
    map: Option<MappedMemory<T>>,
}
impl<T> TryFrom<Arc<Buffer>> for PoolArena<T> {
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
        let inner = PoolArenaInner::new(Layout::from_size_align(
            alloc_size, std::mem::align_of::<T>()).unwrap(), 
            size/alloc_size
        );
        Ok(Self { 
            inner: Cell::new(inner), 
            map, 
            buffer: value.clone() 
        })
    }
}
impl<T> PoolArena<T> {
    pub fn new(device: Arc<LogicalDevice>, info: BufferCreateInfo) -> Result<Self, NightfallError> {
        let buffer = Arc::new(Buffer::new(
            device, 
            info
        ).unwrap());
        Self::try_from(buffer)
    }
    fn inner(&self) -> &mut PoolArenaInner {
        unsafe { self.inner.as_ptr().as_mut().unwrap() }
    }
    pub fn allocate(&self) -> Result<NfPtrType<T>, NightfallError> {
        self.inner().allocate_inner()
        .ok_or(NightfallError::OutOfMemory(
            std::mem::size_of::<T>(), 
            self.inner().size-self.inner().allocations.mul(std::mem::size_of::<T>()
        ))).map(|alloc|{
            let ptr = if let Some(ptr) = self.buffer.get_address() {
                Some(ptr+alloc)
            } else {
                None
            };

            NfPtrType::from(NfPtr::new(self.buffer.handle().as_raw(), alloc, ptr, self.inner().alloc_layout.size()))
        })
    }
    pub fn deallocate(&self, allocation: NfPtr) -> Result<(), NightfallError> {
        self.inner().free_inner(allocation.offset() as usize);
        Ok(())
    }
    pub fn allocated(&self) -> usize {
        self.inner().allocations*self.inner().alloc_layout.size()
    }
    pub fn size(&self) -> usize {
        self.inner().size
    }
    pub fn is_host_mappable(&self) -> Option<std::ptr::NonNull<std::os::raw::c_void>> {
        let ptr = self.map.as_ref()?.ptr() as *mut c_void;
        Some(NonNull::new(ptr).unwrap())
    }
}

impl<T> HostDeviceConversions for PoolArena<T> {
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
/// Creation Information for a [`PoolAllocator`]. The information
/// is similar to [`BufferCreateInfo`] except for the element [`count`]
/// which instead of denoting the size in bytes, represents the size in
/// terms of T in PoolAllocator, so the size is equal to 
/// count*std::mem::size_of::<T>().
#[derive(Default)]
pub struct PoolAllocatorCreateInfo<'a> {
    pub count: usize, 
    pub usage: BufferUsageFlags, 
    pub properties: MemoryPropertyFlags, 
    pub flags: BufferCreateFlags, 
    pub share: Option<&'a[u32]>,
    pub buffer_addressing: bool,
}
pub struct PoolAllocator<T> {
    device: Arc<LogicalDevice>,
    arenas: Cell<Vec<PoolArena<T>>>,
    is_mappable: bool,
}
impl<T> TryFrom<Arc<Buffer>> for PoolAllocator<T> {
    type Error = NightfallError;
    fn try_from(value: Arc<Buffer>) -> Result<Self, Self::Error> {
        let freelist = PoolArena::try_from(value.clone())?;
        Ok(Self { 
            device: value.device(),
            is_mappable: value.is_mappable(),
            arenas: Cell::new(vec![freelist])
        })
    }
}
impl<T> PoolAllocator<T> {
    pub fn new(device: Arc<LogicalDevice>, info: PoolAllocatorCreateInfo) -> Result<Self, NightfallError> {
        let buffer = Arc::new(Buffer::new(
            device, 
            BufferCreateInfo { 
                size: info.count*std::mem::size_of::<T>(), 
                usage: info.usage, 
                properties: info.properties, 
                flags: info.flags, 
                share: info.share, 
                buffer_addressing: info.buffer_addressing
            }
        ).unwrap());
        Self::try_from(buffer)
    }
    pub fn get_pointer_arena(&self, allocation: NfPtr) -> Option<&PoolArena<T>>{
        self.arenas().iter().find(|c|c.buffer.handle() == allocation.buffer())
    }
    fn arenas(&self) -> &mut Vec<PoolArena<T>> {
        let x = unsafe { self.arenas.as_ptr().as_mut().unwrap() };
        x
    }
    fn get_allocatable_arena(&self) -> Option<&PoolArena<T>> {
        let arenas = self.arenas();
        if let Some(freelist) = arenas.iter().find(|arena| arena.size() != 0) {
            Some(freelist)
        } else {
            let buffer = arenas[0].buffer.clone();
            let info = BufferCreateInfo {
                buffer_addressing: buffer.buffer_addressing_enabled(),
                size: buffer.size(),
                usage: buffer.usage(),
                properties: buffer.properties(),
                ..Default::default()
            };
            let arena = PoolArena::new(self.device.clone(), info).ok()?;
            unsafe {
                self.arenas().push(arena);
                arenas.last()
            }
        }
    }
    pub fn allocate(&self) -> Result<NfPtrType<T>, NightfallError> {
        let arena = self.get_allocatable_arena().unwrap();
        arena.allocate()
    }
    pub fn deallocate(&self, allocation: &NfPtrType<T>) -> Result<(), NightfallError> {
        let allocation = allocation.clone().into();
        let arena = self.get_pointer_arena(allocation).unwrap();
        arena.deallocate(allocation)
    }
    pub fn as_device_ptr_ty(&self, allocation: &NfPtrType<T>) -> Option<nightfall_core::memory::DevicePointer> {
        let allocation = allocation.into();
        let arena = self.get_pointer_arena(allocation)?;
        arena.as_device_ptr(allocation)
    }
    pub fn as_host_mut_ptr_ty(&self, allocation: &NfPtrType<T>) -> Option<*mut T> {
        let allocation = allocation.into();
        let arena = self.get_pointer_arena(allocation)?;
        arena.as_host_mut_ptr(allocation).map(|ptr|ptr.cast::<T>())
    }
    pub fn as_host_ptr_ty(&self, allocation: &NfPtrType<T>) -> Option<*const T> {
        let allocation = allocation.into();
        let arena = self.get_pointer_arena(allocation)?;
        arena.as_host_ptr(allocation).map(|ptr|ptr.cast::<T>())
    }
}
impl<T> HostDeviceConversions for PoolAllocator<T> {
    fn as_device_ptr(&self, allocation: NfPtr) -> Option<nightfall_core::memory::DevicePointer> {
        let arena = self.get_pointer_arena(allocation)?;
        arena.as_device_ptr(allocation)
    }
    fn as_host_mut_ptr(&self, allocation: NfPtr) -> Option<*mut std::os::raw::c_void> {
        let arena = self.get_pointer_arena(allocation)?;
        arena.as_host_mut_ptr(allocation)
    }
    fn as_host_ptr(&self, allocation: NfPtr) -> Option<*const std::os::raw::c_void> {
        let arena = self.get_pointer_arena(allocation)?;
        arena.as_host_ptr(allocation)
    }
}

#[cfg(test)]
mod tests {
    use crate::{PoolArena, PoolAllocator};

    #[test]
    fn test_pool_allocator() {
        use nightfall_core::{
            buffers::{BufferCreateInfo, BufferUsageFlags, MemoryPropertyFlags},
            device::LogicalDeviceBuilder, 
            instance::InstanceBuilder, 
            queue::DeviceQueueCreateFlags, 
        };
        use crate::FreeListAllocator;
        pub struct AABB {
            pub pos_x: f32,
            pub pos_y: f32,
            pub width: f32,
            pub height: f32,
        }
        let instance = InstanceBuilder::new().build().unwrap();
        let physical_device = instance.enumerate_physical_devices().unwrap().next().unwrap();
        // doesn't matter for this test
        let queue_family_index = 0;
        let (device, mut queues) = LogicalDeviceBuilder::new()
            .add_queue(DeviceQueueCreateFlags::empty(), queue_family_index as u32, 1, 0, &1.0)
            .build(physical_device.clone()).unwrap();
        let pool = PoolAllocator::<AABB>::new(device.clone(), crate::PoolAllocatorCreateInfo { 
            count: 250000, // 250000*std::mem::size_of::<AABB>() = 4 mb
            usage: BufferUsageFlags::STORAGE_BUFFER, 
            properties: MemoryPropertyFlags::HOST_VISIBLE|MemoryPropertyFlags::HOST_COHERENT,
            ..Default::default() 
        }).unwrap();
        let aabb = pool.allocate().unwrap();
        let aabb_ptr = pool.as_host_mut_ptr_ty(&aabb).unwrap();
        unsafe {
            *aabb_ptr = AABB {
                pos_x: 1.0,
                pos_y: 2.0,
                width: 3.0,
                height: 4.0,
            };
            assert!((*aabb_ptr).pos_x == 1.0);
            assert!((*aabb_ptr).pos_y == 2.0);
            assert!((*aabb_ptr).width == 3.0);
            assert!((*aabb_ptr).height == 4.0);
        }
        pool.deallocate(&aabb);
    }
    #[test]
    fn test_pool_overflow() {
        use nightfall_core::{
            buffers::{BufferCreateInfo, BufferUsageFlags, MemoryPropertyFlags},
            device::LogicalDeviceBuilder, 
            instance::InstanceBuilder, 
            queue::DeviceQueueCreateFlags, 
        };
        use crate::FreeListAllocator;
        pub struct AABB {
            pub pos_x: f32,
            pub pos_y: f32,
            pub width: f32,
            pub height: f32,
        }
        let instance = InstanceBuilder::new().build().unwrap();
        let physical_device = instance.enumerate_physical_devices().unwrap().next().unwrap();
        // doesn't matter for this test
        let queue_family_index = 0;
        let (device, mut queues) = LogicalDeviceBuilder::new()
            .add_queue(DeviceQueueCreateFlags::empty(), queue_family_index as u32, 1, 0, &1.0)
            .build(physical_device.clone()).unwrap();
        let pool = PoolAllocator::<AABB>::new(device.clone(), crate::PoolAllocatorCreateInfo { 
            count: 50, // 250000*std::mem::size_of::<AABB>() = 4 mb
            usage: BufferUsageFlags::STORAGE_BUFFER, 
            properties: MemoryPropertyFlags::HOST_VISIBLE|MemoryPropertyFlags::HOST_COHERENT,
            ..Default::default() 
        }).unwrap();
        let aabb = pool.allocate().unwrap();
        let aabb_ptr = pool.as_host_mut_ptr_ty(&aabb).unwrap();
        unsafe {
            *aabb_ptr = AABB {
                pos_x: 1.0,
                pos_y: 2.0,
                width: 3.0,
                height: 4.0,
            };
            assert!((*aabb_ptr).pos_x == 1.0);
            assert!((*aabb_ptr).pos_y == 2.0);
            assert!((*aabb_ptr).width == 3.0);
            assert!((*aabb_ptr).height == 4.0);
        }
    }
}