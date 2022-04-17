use static_assertions::{const_assert_eq, const_assert_ne};
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
    pub fn new(size_bytes: usize) -> Self {
        debug_assert_ne!(size_bytes, 0, "Cannot create an allocator with size 0");

        // align shouldn't be 0
        const_assert_ne!(L1_CACHE_LINE_SIZE, 0);
        // align should be a power of two
        const_assert_eq!(L1_CACHE_LINE_SIZE & (L1_CACHE_LINE_SIZE - 1), 0);
        // Since we check align ourselves, this should only fail on overflow.
        let layout = Layout::from_size_align(size_bytes, L1_CACHE_LINE_SIZE)
            .expect("Failed to create memory layout");
        let block_start = unsafe { std::alloc::alloc(layout) };

        if block_start.is_null() {
            std::alloc::handle_alloc_error(layout);
        }

        Self {
            block_start,
            layout,
            size_bytes,
            next_alloc: Cell::new(block_start),
        }
    }
}

impl Drop for LinearAllocator {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(self.block_start, self.layout);
        }
    }
}

pub trait AllocatorInternal {
    // Interior mutability required by interface
    // The references will be to non-overlapping memory as long as [rewind()] is not misused.
    #[allow(clippy::mut_from_ref)]
    /// Allocates and initializes `obj`
    fn alloc_internal<T: Sized>(&self, obj: T) -> &mut T;

    /// Rewinds the allocator back to `alloc`.
    /// # Safety
    ///  - `alloc` has to be a pointer to an allocation from [alloc_internal()]
    ///     or a pointer returned by [peek()].
    ///  - Caller is responsible for calling drop on objects returned by
    ///    [alloc_internal()] that will be rewound over, if they don't implement Copy
    ///  - Caller also needs to ensure that any references held to the rewound
    ///    objects are dropped
    unsafe fn rewind(&self, alloc: *mut u8);

    /// Returns the pointer to the start of the free block
    fn peek(&self) -> *mut u8;
}

impl AllocatorInternal for LinearAllocator {
    #[allow(clippy::mut_from_ref)]
    fn alloc_internal<T: Sized>(&self, obj: T) -> &mut T {
        let size_bytes = std::mem::size_of::<T>();
        let alignment = std::mem::align_of::<T>();

        let next_alloc = self.next_alloc.get();
        let align_offset = next_alloc.align_offset(alignment);

        let previous_size = unsafe { next_alloc.offset_from(self.block_start) as usize };
        let new_size = previous_size + align_offset + size_bytes;
        if new_size > self.size_bytes {
            let remaining_bytes = self.size_bytes - previous_size;
            panic!(
                "Tried to allocate {} bytes aligned at {} with only {} remaining.",
                size_bytes, alignment, remaining_bytes
            );
        }

        let new_alloc = unsafe { self.next_alloc.get().add(align_offset) };

        self.next_alloc
            .replace(unsafe { new_alloc.add(size_bytes) });

        unsafe {
            let t_ptr = new_alloc as *mut T;
            t_ptr.write(obj);
            &mut *t_ptr
        }
    }

    unsafe fn rewind(&self, alloc: *mut u8) {
        // Let's be nice and catch the obvious error
        // For non-PoD struct dtor validation, we are out of luck
        debug_assert!(
            (alloc as usize) >= (self.block_start as usize)
                && (alloc as usize) < (self.block_start as usize) + self.size_bytes,
            "alloc doesn't belong to this allocator"
        );
        self.next_alloc.replace(alloc);
    }

    fn peek(&self) -> *mut u8 {
        self.next_alloc.get()
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::mem::{align_of, drop, size_of};

    #[test]
    fn alloc_u8() {
        let alloc = LinearAllocator::new(1024);

        let a = alloc.alloc_internal(0xABu8);
        assert_eq!(*a, 0xABu8);
        assert_eq!(a as *const u8, alloc.block_start);
        assert_eq!(
            unsafe { alloc.next_alloc.get().offset_from(alloc.block_start) },
            size_of::<u8>() as isize
        );
    }

    #[test]
    fn alloc_pod() {
        let alloc = LinearAllocator::new(1024);

        #[allow(dead_code)]
        struct A {
            data: u32,
        }

        let a = alloc.alloc_internal(A { data: 0xDEADC0DE });
        assert_eq!(a.data, 0xDEADC0DE);
        assert_eq!(a as *const A as *const u8, alloc.block_start);
        assert_eq!(
            unsafe { alloc.next_alloc.get().offset_from(alloc.block_start) },
            size_of::<A>() as isize
        );
    }

    #[test]
    fn alloc_drop() {
        let alloc = LinearAllocator::new(1024);

        #[allow(dead_code)]
        struct A {
            data: Vec<u32>,
        }

        let a = alloc.alloc_internal(A {
            data: vec![0xC0FFEEEE],
        });
        assert_eq!(a.data.len(), 1);
        assert_eq!(a.data[0], 0xC0FFEEEE);
        assert_eq!(a as *const A as *const u8, alloc.block_start);
        assert_eq!(
            unsafe { alloc.next_alloc.get().offset_from(alloc.block_start) },
            size_of::<A>() as isize
        );

        drop(a);
    }

    #[test]
    fn two_allocs() {
        let alloc = LinearAllocator::new(1024);

        let a = alloc.alloc_internal(0xCAFEBABEu32);
        let b = alloc.alloc_internal(0xDEADCAFEu32);
        let a_ptr = a as *const u32;
        let b_ptr = b as *const u32;
        assert_eq!(*a, 0xCAFEBABEu32);
        assert_eq!(*b, 0xDEADCAFEu32);
        assert_eq!(unsafe { b_ptr.offset_from(a_ptr) }, 1);
        assert_eq!(
            unsafe { alloc.next_alloc.get().offset_from(alloc.block_start) },
            size_of::<u32>() as isize * 2
        );
    }

    #[should_panic(
        expected = "Tried to allocate 1025 bytes aligned at 1 with only 1024 remaining."
    )]
    #[test]
    fn overflow_first() {
        let alloc = LinearAllocator::new(1024);
        let _ = alloc.alloc_internal([0u8; 1025]);
    }

    #[should_panic(expected = "Tried to allocate 1000 bytes aligned at 4 with only 768 remaining.")]
    #[test]
    fn overflow_second() {
        let alloc = LinearAllocator::new(1024);
        let _ = alloc.alloc_internal([0u8; 256]);
        let _ = alloc.alloc_internal([0u32; 250]);
    }

    #[test]
    fn different_alignment() {
        let alloc = LinearAllocator::new(1024);

        #[allow(dead_code)]
        #[repr(C)]
        struct A {
            data: u8,
        }
        #[allow(dead_code)]
        #[repr(C)]
        struct B {
            data: u64,
        }
        assert_ne!(size_of::<A>() % align_of::<B>(), 0);

        let _ = alloc.alloc_internal(A { data: 0 });
        let b = alloc.alloc_internal(B { data: 0 });
        assert_eq!((b as *const B as usize) % align_of::<B>(), 0);
    }

    #[test]
    fn rewind() {
        let alloc = LinearAllocator::new(1024);

        let _ = alloc.alloc_internal(0u8);
        let target = alloc.peek();
        let _ = alloc.alloc_internal(0u64);
        assert_ne!(alloc.next_alloc.get(), target);
        unsafe { alloc.rewind(target) };
        assert_eq!(alloc.next_alloc.get(), target);
    }

    #[should_panic(expected = "alloc doesn't belong to this allocator")]
    #[test]
    fn rewind_assert_below() {
        let alloc = LinearAllocator::new(1024);
        unsafe { alloc.rewind(0x1 as *mut u8) };
    }

    #[should_panic(expected = "alloc doesn't belong to this allocator")]
    #[test]
    fn rewind_assert_above() {
        let alloc = LinearAllocator::new(1024);
        unsafe { alloc.rewind(alloc.peek().offset(1024)) }
    }
}
