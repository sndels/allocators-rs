use static_assertions::{const_assert_eq, const_assert_ne};
use std::{alloc::Layout, cell::Cell};

pub struct LinearAllocator {
    block_start: *mut u8,
    layout: Layout,
    size_bytes: usize,
    // Interior mutability because alloc_internal() and rewind() need to work on
    // immutable references so that we can allocate multiple objects
    next_alloc: Cell<*mut u8>,
}

// This applies for most ARM, x86 and x64, but notably not for Apple M1 that has 128B lines
const L1_CACHE_LINE_SIZE: usize = 64;

impl LinearAllocator {
    pub fn new(size_bytes: usize) -> Self {
        assert_ne!(size_bytes, 0, "Cannot create an allocator with size 0");
        // Limit so that we can assume allocation arithmetic can never overflow
        assert!(size_bytes < isize::MAX as usize);

        const ALIGN: usize = L1_CACHE_LINE_SIZE;
        // align shouldn't be 0
        const_assert_ne!(ALIGN, 0);
        // align should be a power of two
        const_assert_eq!(ALIGN & (ALIGN - 1), 0);
        // Since we check align ourselves, this should only fail on overflow.
        let layout =
            Layout::from_size_align(size_bytes, ALIGN).expect("Failed to create memory layout");

        // Safety:
        // - layout has a non-zero size since size_bytes is not 0 and its construction succeeded
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
        // Safety:
        //  - self.block_start was allocated using the same allocator in new()
        //  - self.layout is the layout it was allocated with
        unsafe {
            std::alloc::dealloc(self.block_start, self.layout);
        }
    }
}

// This interface is not exposed outside the library with the goal of being safe all around
pub trait LinearAllocatorInternal {
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

impl LinearAllocatorInternal for LinearAllocator {
    #[allow(clippy::mut_from_ref)]
    fn alloc_internal<T: Sized>(&self, obj: T) -> &mut T {
        let size_bytes = std::mem::size_of::<T>();
        let alignment = std::mem::align_of::<T>();
        // Make sure new_size never overflows
        // size is always a multiple of alignment
        assert!(size_bytes < (isize::MAX / 2) as usize);

        let next_alloc = self.next_alloc.get();
        let align_offset = next_alloc.align_offset(alignment);
        assert_ne!(align_offset, usize::MAX);

        // Safety:
        // - self.block_start is at the start of the allocation and next_alloc
        //   has been verified to be within the allocation (or one byte past it)
        //   either by alloc_internal() or rewind()
        // - We assume next_alloc is derived from self.block_start because it's either
        //   - the same as self.block_start
        //   - derived from a previous self.next_alloc
        //   - from rewind() that has safety rules expecting the input to be
        //     - from peek()
        //       - some previous self.next_alloc
        //     - pointer to an object from alloc_internal()
        //       - derived from some previous self.next_alloc
        // - Distance between two *mut u8 is always a multiple of u8
        // - Maximum held block size is under isize::MAX so distances within it can't overflow isize
        // - Rust allocations never wrap around the address space
        let previous_size = unsafe { next_alloc.offset_from(self.block_start) as usize };

        // The asserts above make sure this can't overflow since
        // previous_size <= self.size_bytes < isize::MAX
        let new_size = previous_size + align_offset + size_bytes;
        if new_size > self.size_bytes {
            let remaining_bytes = self.size_bytes - previous_size;
            panic!(
                "Tried to allocate {} bytes aligned at {} with only {} remaining.",
                size_bytes, alignment, remaining_bytes
            );
        }

        // Safety:
        // - self.next_alloc has been verified to be within the allocation either
        //   by alloc_internal() or rewind(), and we just verified that the aligned
        //   object fits the allocation
        // - Maximum held block size is under isize::MAX so offsets within it can't overflow isize
        // - Rust allocations never wrap around the address space
        let new_alloc = unsafe {
            let new_alloc = self.next_alloc.get().add(align_offset);
            self.next_alloc.replace(new_alloc.add(size_bytes));
            new_alloc
        };

        // Safety:
        // - new_alloc is a pointer to at least size_of::<T>() bytes of the block
        //   from self.block_start and this allocator can't shared between threads
        // - We aligned new_alloc for T
        unsafe {
            let t_ptr = new_alloc as *mut T;
            t_ptr.write(obj);
            &mut *t_ptr
        }
    }

    unsafe fn rewind(&self, alloc: *mut u8) {
        // Let's be nice and catch the obvious error
        // Reference lifetimes and allocated structs needing Drop are truly the
        // responsibility of the caller
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
