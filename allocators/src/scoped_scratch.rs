use crate::linear_allocator::{LinearAllocator, LinearAllocatorInternal};

use std::cell::{Cell, RefCell};

// Inspired by Frostbite's Scope Stack Allocation
// Runtime asserts that only the innermost scope is used
// Perf impact seems negligible for scope alloc, drop and individual allocs

struct ScopeData<'a> {
    mem: *mut u8,
    dtor: Option<&'a dyn Fn(*mut u8)>,
    previous: Option<&'a ScopeData<'a>>,
}

pub struct ScopedScratch<'a, 'b> {
    allocator: &'a LinearAllocator,
    alloc_start: *mut u8,
    data_chain: Cell<Option<&'a ScopeData<'a>>>,
    parent_locked: Option<&'b RefCell<bool>>,
    locked: RefCell<bool>,
}

impl Drop for ScopedScratch<'_, '_> {
    fn drop(&mut self) {
        self.iter_chain(&mut |scope| {
            if let Some(dtor) = scope.dtor {
                dtor(scope.mem)
            }
        });

        // # Safety
        //  - self.alloc_start is from self.allocator.peek() at the start of the scope
        //  - dtors for the objects that require it in this scope were just called
        //    - lock assertions ensure only the innermost scope is ever used
        //  - Any references to objects in this scope are limited by its lifetime
        unsafe {
            self.allocator.rewind(self.alloc_start);
        }

        if let Some(parent_locked) = self.parent_locked {
            *parent_locked.borrow_mut() = false;
        }
    }
}

impl<'a, 'b> ScopedScratch<'a, 'b> {
    pub fn new(allocator: &'a mut LinearAllocator) -> Self {
        Self {
            allocator,
            alloc_start: allocator.peek(),
            data_chain: Cell::new(None),
            parent_locked: None,
            locked: RefCell::new(false),
        }
    }

    pub fn new_scope(&'b self) -> ScopedScratch<'a, 'b> {
        *self.locked.borrow_mut() = true;
        Self {
            allocator: self.allocator,
            alloc_start: self.allocator.peek(),
            data_chain: Cell::new(None),
            parent_locked: Some(&self.locked),
            locked: RefCell::new(false),
        }
    }

    // Interior mutability required by interface
    // The references will be to non-overlapping memory as the allocator is only
    // rewound on drop
    #[allow(clippy::mut_from_ref)]
    /// Allocates `obj` with the held allocator. If `obj` needs Drop, its destruction
    /// is added to internal bookkeeping and is handled when this `ScopeScratch` is dropped.
    pub fn alloc<T: Sized>(&self, obj: T) -> &mut T {
        assert!(
            !*self.locked.borrow(),
            "Tried to allocate from a ScopedScratch that has an active child scope"
        );

        // The compiler seems smart enough that this check is optimized out
        if !std::mem::needs_drop::<T>() {
            return self.allocator.alloc_internal(obj);
        }

        let mut data = self.allocator.alloc_internal(ScopeData {
            mem: std::ptr::null_mut::<u8>(),
            dtor: Some(&|ptr: *mut u8| unsafe { (ptr as *mut T).drop_in_place() }),
            previous: self.data_chain.get(),
        });

        let ret = self.allocator.alloc_internal(obj);
        data.mem = (ret as *mut T) as *mut u8;
        self.data_chain.replace(Some(data));
        ret
    }

    #[cfg(test)]
    pub fn data_chain_len(&self) -> usize {
        let mut len = 0;
        self.iter_chain(&mut |_| len += 1);
        len
    }

    fn iter_chain(&self, f: &mut dyn FnMut(&ScopeData)) {
        let mut data_chain = self.data_chain.get();
        while let Some(scope) = data_chain {
            f(scope);
            data_chain = scope.previous;
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn alloc_primitive() {
        let mut alloc = LinearAllocator::new(1024);
        let scratch = ScopedScratch::new(&mut alloc);

        let a = scratch.alloc(0xABu8);
        assert_eq!(*a, 0xABu8);
    }

    #[test]
    fn alloc_pod() {
        let mut alloc = LinearAllocator::new(1024);
        let scratch = ScopedScratch::new(&mut alloc);

        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        struct A {
            data: u32,
        }

        let a = scratch.alloc(A {
            data: 0xDEADC0DEu32,
        });
        assert_eq!(a.data, 0xDEADC0DEu32);
    }

    #[test]
    fn alloc_obj() {
        let mut alloc = LinearAllocator::new(1024);
        let scratch = ScopedScratch::new(&mut alloc);

        #[allow(dead_code)]
        struct A {
            data: Vec<u32>,
        }

        let a = scratch.alloc(A {
            data: vec![0xC0FFEEEEu32],
        });
        assert_eq!(a.data.len(), 1);
        assert_eq!(a.data[0], 0xC0FFEEEEu32);
    }

    #[test]
    fn scope_rewind() {
        let mut alloc = LinearAllocator::new(1024);
        let start_ptr = alloc.peek();
        {
            let scratch = ScopedScratch::new(&mut alloc);
            let _ = scratch.alloc(0u32);
        }
        assert_eq!(start_ptr, alloc.peek());
    }

    #[test]
    fn new_scope() {
        let mut alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopedScratch::new(&mut alloc);
            let a = scratch.alloc(0xCAFEBABEu32);
            assert_eq!(*a, 0xCAFEBABEu32);
            {
                let scratch2 = scratch.new_scope();
                let b = scratch2.alloc(0xDEADCAFEu32);
                assert_eq!(*b, 0xDEADCAFEu32);
            }
            assert_eq!(*a, 0xCAFEBABEu32);
            let b = scratch.alloc(0xC0FFEEEEu32);
            assert_eq!(*b, 0xC0FFEEEEu32);
        }
    }

    #[should_panic(
        expected = "Tried to allocate from a ScopedScratch that has an active child scope"
    )]
    #[test]
    fn active_parent_alloc() {
        let mut alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopedScratch::new(&mut alloc);
            let _ = scratch.alloc(0xCAFEBABEu32);
            {
                let _scratch2 = scratch.new_scope();
                let _ = scratch.alloc(0xDEADCAFEu32);
            }
        }
    }

    #[test]
    fn no_drop() {
        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        struct A {
            data: u32,
        }

        let mut alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopedScratch::new(&mut alloc);

            let _ = scratch.alloc(A {
                data: 0xC0FFEEEEu32,
            });
            let _ = scratch.alloc(A {
                data: 0xDEADC0DEu32,
            });
            assert_eq!(scratch.data_chain_len(), 0);
        }
    }

    #[test]
    fn drop_order() {
        struct A<'a> {
            data: u32,
            dtor_push: &'a mut dyn FnMut(u32) -> (),
        }
        impl<'a> Drop for A<'a> {
            fn drop(&mut self) {
                (self.dtor_push)(self.data);
            }
        }

        let mut dtor_data: Vec<u32> = vec![];
        let mut dtor_push = |v| dtor_data.push(v);

        let mut alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopedScratch::new(&mut alloc);

            let _ = scratch.alloc(A {
                data: 0xCAFEBABEu32,
                dtor_push: &mut dtor_push,
            });
            let _ = scratch.alloc(A {
                data: 0xDEADCAFEu32,
                dtor_push: &mut dtor_push,
            });
            assert_eq!(scratch.data_chain_len(), 2);
        }
        assert_eq!(dtor_data.len(), 2);
        assert_eq!(dtor_data[0], 0xDEADCAFEu32);
        assert_eq!(dtor_data[1], 0xCAFEBABEu32);
    }

    #[test]
    fn drop_some() {
        struct A<'a> {
            data: u32,
            dtor_push: &'a mut dyn FnMut(u32) -> (),
        }
        impl<'a> Drop for A<'a> {
            fn drop(&mut self) {
                (self.dtor_push)(self.data);
            }
        }

        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        struct B {
            data: u32,
        }

        let mut dtor_data: Vec<u32> = vec![];
        let mut dtor_push = |v| dtor_data.push(v);

        let mut alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopedScratch::new(&mut alloc);

            let _ = scratch.alloc(A {
                data: 0xCAFEBABEu32,
                dtor_push: &mut dtor_push,
            });
            let _ = scratch.alloc(B {
                data: 0xC0FFEEEEu32,
            });
            let _ = scratch.alloc(A {
                data: 0xDEADCAFEu32,
                dtor_push: &mut dtor_push,
            });
            let _ = scratch.alloc(B {
                data: 0xDEADC0DEu32,
            });
            assert_eq!(scratch.data_chain_len(), 2);
        }
        assert_eq!(dtor_data.len(), 2);
        assert_eq!(dtor_data[0], 0xDEADCAFEu32);
        assert_eq!(dtor_data[1], 0xCAFEBABEu32);
    }
}
