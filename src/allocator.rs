use std::{alloc::Layout, cell::Cell};

pub struct LinearAllocator {
    block_start: *mut u8,
    layout: Layout,
    size_bytes: usize,
    next_alloc: Cell<*mut u8>,
}

// TODO: Do we care to expose this?
const L1_CACHE_LINE_SIZE: usize = 64;

impl LinearAllocator {
    pub fn new(size_bytes: usize) -> Result<Self, CreateError> {
        // println!("LinearAllocator::new({})", size_bytes);
        if size_bytes == 0 {
            return Err(CreateError::ZeroBytes(
                "Cannot create allocator with size 0".into(),
            ));
        }

        let layout = Layout::from_size_align(size_bytes, L1_CACHE_LINE_SIZE).map_err(|err| {
            CreateError::Layout(format!("Failed to create memory layout: {:?}", err))
        })?;

        let block_start = unsafe { std::alloc::alloc(layout) };

        if block_start.is_null() {
            // TODO: For what reason?
            return Err(CreateError::BackingAllocation(
                "Backing allocation failed for some reason".into(),
            ));
        }

        Ok(Self {
            block_start,
            layout,
            size_bytes,
            next_alloc: Cell::new(block_start),
        })
    }
}

impl Drop for LinearAllocator {
    fn drop(&mut self) {
        // println!("LinearAllocator::drop()");
        unsafe {
            std::alloc::dealloc(self.block_start, self.layout);
        }
    }
}

pub trait AllocatorInternal {
    fn alloc_internal<T>(&self, obj: T) -> Result<&mut T, AllocationError>;
    unsafe fn rewind(&self, alloc: *mut u8);
    fn peek(&self) -> *mut u8;
}

impl AllocatorInternal for LinearAllocator {
    fn alloc_internal<T>(&self, obj: T) -> Result<&mut T, AllocationError> {
        let size_bytes = std::mem::size_of::<T>();
        let alignment = std::mem::align_of::<T>();
        // println!("size {}", size_bytes);

        let next_alloc = self.next_alloc.get();
        let align_offset = next_alloc.align_offset(alignment);

        let previous_size = unsafe { next_alloc.offset_from(self.block_start) as usize };
        let new_size = previous_size + align_offset + size_bytes;
        if new_size > self.size_bytes {
            let remaining_bytes = self.size_bytes - previous_size;
            return Err(AllocationError::OutOfMemory(format!(
                "Tried to allocate {} bytes aligned at {} with only {} remaining.",
                size_bytes, alignment, remaining_bytes
            )));
        }

        let new_alloc = unsafe { self.next_alloc.get().add(align_offset) };

        self.next_alloc
            .replace(unsafe { new_alloc.add(size_bytes) });

        Ok(unsafe {
            let t_ptr = new_alloc as *mut T;
            t_ptr.write(obj);
            &mut *t_ptr
        })
    }

    /// Rewinds the allocator back to `alloc`.
    /// # Safety
    ///  - `alloc` has to be a pointer allocated by this Allocator.
    unsafe fn rewind(&self, alloc: *mut u8) {
        self.next_alloc.replace(alloc);
    }

    fn peek(&self) -> *mut u8 {
        self.next_alloc.get()
    }
}

#[derive(Debug)]
pub enum AllocationError {
    OutOfMemory(String),
}

#[derive(Debug)]
pub enum CreateError {
    ZeroBytes(String),
    Layout(String),
    BackingAllocation(String),
}
