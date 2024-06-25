fn main() {
    use std::alloc::Layout;
    use nightfall_core::{
        buffers::{BufferCreateInfo, BufferUsageFlags, MemoryPropertyFlags},
        device::LogicalDeviceBuilder, 
        instance::InstanceBuilder, 
        queue::DeviceQueueCreateFlags, 
    };
    use starlit_alloc::{FreeListAllocator, GeneralAllocator, HostDeviceConversions};
    let instance = InstanceBuilder::new().build().unwrap();
    let physical_device = instance.enumerate_physical_devices().unwrap().next().unwrap();
    // doesn't matter for this test
    let queue_family_index = 0;
    let (device, _) = LogicalDeviceBuilder::new()
        .add_queue(DeviceQueueCreateFlags::empty(), queue_family_index as u32, 1, 0, &1.0)
        .build(physical_device.clone()).unwrap();
    let freelist = FreeListAllocator::new(device.clone(), BufferCreateInfo {
        buffer_addressing: true,
        usage: BufferUsageFlags::STORAGE_BUFFER,
        size: 64000000, // 64 mb
        properties: MemoryPropertyFlags::HOST_VISIBLE|MemoryPropertyFlags::HOST_COHERENT,
        ..Default::default()
    }).unwrap();
    let layout = Layout::new::<i32>();
    let allocation = freelist.allocate(layout).unwrap();
    let host = freelist.as_host_mut_ptr(allocation).unwrap().cast::<i32>();
    unsafe {
        *host = 10;
        println!("{}", *host);
    }
    // All allocations must be deallocated otherwise a memory leak will occur.
    freelist.deallocate(allocation, layout).unwrap();
}