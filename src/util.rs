use std::sync::Arc;

use nightfall_core::{device::{LogicalDevice, LogicalDeviceBuilder}, instance::InstanceBuilder, queue::{DeviceQueueCreateFlags, Queue, QueueFlags}, Version};

pub fn standard() -> (Arc<LogicalDevice>, impl ExactSizeIterator<Item = Arc<Queue>>) {
    let instance = InstanceBuilder::new()
        .set_version(Version::new(1, 3, 0))
        .validation_layers()
        .get_physical_device_properties2()
        .device_group_creation_extension()
        .build().unwrap();
        let physical_device = instance.enumerate_physical_devices().unwrap().next().unwrap();
        let queue_family_index = physical_device.enumerate_queue_family_properties()
        .iter()
        .enumerate()
        .position(|(_queue_family_index, queue_family_properties)|{
            queue_family_properties.queue_flags.contains(QueueFlags::COMPUTE | QueueFlags::GRAPHICS)
        }).unwrap();
        let (device, mut queues) = LogicalDeviceBuilder::new()
            .add_queue(DeviceQueueCreateFlags::empty(), queue_family_index as u32, 1, 0, &1.0)
            .enable_buffer_addressing()
            .device_group()
            .enable_float64()
            .descriptor_indexing(
                true, 
                true, 
                true, 
                true, 
                true, 
                true, 
                true, 
                true, 
                true)
            .subgroup_ballot()
            .build(physical_device.clone()).unwrap();
    (device, queues)
}