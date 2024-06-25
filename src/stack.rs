use std::{cell::Cell, os::raw::c_void, sync::Arc};

use ash::vk::Handle;
use nightfall_core::{buffers::{Buffer, BufferCreateInfo, MappedMemory}, device::LogicalDevice, memory::DevicePointer, NfPtr};

use crate::{HostDeviceConversions, StarlitAllocError};

pub struct StackAllocatorInner {
    granularity: usize,
    size: usize,
    sp: usize,
}
impl StackAllocatorInner {
    pub fn new(size: usize, granularity: usize) -> Self {
        Self {
            granularity,
            size,
            sp: 0
        }
    }
    // fails when theres not enough memory to allocate
    pub fn push(&mut self, size: usize) -> Option<usize> {
        if self.sp >= self.size {
            None
        } else {
            let base = self.sp;
            self.sp += size;
            Some(base)
        }
    }
    pub fn pop(&mut self, size: usize) {
        if self.sp < size {
            self.sp = 0
        } else {
            self.sp -= size;
        }
    }
}
pub struct StackFrame<'a> {
    stack: &'a Cell<StackAllocatorInner>,
    size: usize
}
impl<'a> Drop for StackFrame<'a> {
    fn drop(&mut self) {
        let stack = unsafe { self.stack.as_ptr().as_mut().unwrap() };
        stack.pop(self.size);
    }
}
pub struct StackAllocator {
    stack: Cell<StackAllocatorInner>,
    buffer: Arc <Buffer>,
    map: Option<MappedMemory<u8>>,
}

impl TryFrom<Arc<Buffer>> for StackAllocator {
    type Error = StarlitAllocError;
    fn try_from(value: Arc<Buffer>) -> Result<Self, Self::Error> {
        let size = value.size();
        let range = 0..value.size();
        let map = if value.is_mappable() {
            Some(value.clone().map_memory::<u8>(range).unwrap())
        } else {
            None
        };
        let stack = Cell::new(StackAllocatorInner::new(size, 16));
        Ok(Self { 
            stack, 
            map, 
            buffer: value.clone() 
        })
    }
}
impl StackAllocator {
    pub fn new(device: Arc<LogicalDevice>, info: BufferCreateInfo) -> Result<Self, StarlitAllocError> {
        let buffer = Arc::new(Buffer::new(
            device, 
            info
        ).unwrap());
        Self::try_from(buffer)
    }
    // deallocations are performed by StackFrame
    pub fn push(&self, size: usize) -> Result<(NfPtr, StackFrame), StarlitAllocError> {
        let base = {
            let stack = unsafe { self.stack.as_ptr().as_mut().unwrap() };
            stack.push(size).ok_or(StarlitAllocError::OutOfMemory {
                size,
                free_size: stack.size-stack.sp
            })?
        };
        let ptr = if let Some(ptr) = self.buffer.get_address() {
            Some(ptr+base)
        } else {
            None
        };
        Ok((NfPtr::new(self.buffer.handle().as_raw(), base, ptr, size), StackFrame {
            stack: &self.stack,
            size
        }))
    }
}

impl HostDeviceConversions for StackAllocator {
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
            Some(DevicePointer::from_raw((addr.addr() + allocation.offset()) as u64))
        } else {
            None
        }
    }
}