use thiserror::Error;

#[derive(Error, Debug, PartialEq, PartialOrd)]
pub enum StarlitAllocError {
    #[error("{0}")]
    InternalError(String),
    #[error("Failed to allocate memory of size {size}, there is {free_size} remaining memory")]
    OutOfMemory {
        size: usize,
        free_size: usize,
    },
    #[error("An allocation of size 0 is not allowed")]
    ZeroSize,
    #[error("Failed to deallocate memory which is not marked as in use by the allocator")]
    InvalidFree,
    #[error("Attempted to use memory on the host but it is not marked as host visible.")]
    MemoryWriteError,
}