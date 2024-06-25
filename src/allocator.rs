use std::{cell::Cell, collections::HashMap, sync::Arc};

use nightfall_core::{buffers::{BufferCreateInfo, BufferUsageFlags, MemoryPropertyFlags}, queue::Queue};

use crate::{FreeListArena, FreeListAllocator, GeneralAllocator, HostDeviceConversions, StackAllocator, StarlitAllocError};
/// The GpuProgramState contains all memory related things.
pub trait GpuAllocators {
    fn freelist(&self, usage: BufferUsageFlags, properties: MemoryPropertyFlags) -> Option<Arc<dyn GeneralAllocator>>;
}
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord,  Hash)]
struct BufferUsageAndMemoryPropertyFlags(u64);
impl BufferUsageAndMemoryPropertyFlags {
    pub fn new(usage: BufferUsageFlags, properties: MemoryPropertyFlags) -> Self {
        Self(usage.as_raw() as u64 | ((properties.as_raw() as u64) << 32))
    }
}
pub struct StandardAllocator {
    transfer: Arc<Queue>,
    freelists: Cell<HashMap<BufferUsageAndMemoryPropertyFlags, Arc<dyn GeneralAllocator>>>,
}

impl StandardAllocator {
    pub fn new(transfer: Arc<Queue>) -> Result<Arc<Self>, StarlitAllocError> {
        Ok(Arc::new(Self {
            transfer, 
            // stack_host, 
            // stack_device, 
            freelists: Cell::new(HashMap::new()),
        }))
    }
}

impl GpuAllocators for StandardAllocator {
    fn freelist(&self, usage: BufferUsageFlags, properties: MemoryPropertyFlags) -> Option<Arc<dyn GeneralAllocator>> {
        let key = BufferUsageAndMemoryPropertyFlags::new(usage, properties);
        let freelists = unsafe { self.freelists.as_ptr().as_mut().unwrap() };
        if let Some(freelist) = unsafe { freelists.get(&key) } {
            Some(freelist.clone())
        } else {
            freelists.insert(key, Arc::new(FreeListAllocator::new(self.transfer.device(), BufferCreateInfo {
                buffer_addressing: usage.contains(BufferUsageFlags::SHADER_DEVICE_ADDRESS),
                properties,
                size: 64000000,
                usage,
                ..Default::default()
            }).unwrap()));
            freelists.get(&key).cloned()
        }
    }
}