use crate::allocator::{AllocatorInternal, LinearAllocator};

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
        let mut data_chain = self.data_chain.get();
        while let Some(scope) = data_chain {
            if let Some(dtor) = scope.dtor {
                dtor(scope.mem)
            }
            data_chain = scope.previous;
        }

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
    pub fn new(allocator: &'a LinearAllocator) -> Self {
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
    // TODO: Can we get away with no Drop?
    //       Aggregate can have no Drop of its own but store data that implements it.
    //       How does drop_in_place behave then?
    pub fn new_obj<T>(&self, obj: T) -> &mut T {
        assert!(
            !*self.locked.borrow(),
            "Tried to allocate from a ScopedScratch that has an active child scope"
        );

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

    // Interior mutability required by interface
    // The references will be to non-overlapping memory as the allocator is only
    // rewound on drop
    #[allow(clippy::mut_from_ref)]
    // Safety bounds on allocation, approximate true PoD
    //       Copy - won't have Drop
    // TODO: Could this be abstracted such that we could call one method for both
    //       and let the compiler do magic to figure out which it is? Sounds like specialization but for param type.
    pub fn new_pod<T: Copy>(&self, pod: T) -> &mut T {
        assert!(
            !*self.locked.borrow(),
            "Tried to allocate from a ScopedScratch that has an active child scope"
        );

        self.allocator.alloc_internal(pod)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn alloc_primitive() {
        let alloc = LinearAllocator::new(1024);
        let scratch = ScopedScratch::new(&alloc);

        let a = scratch.new_pod(0xABu8);
        assert_eq!(*a, 0xABu8);
    }

    #[test]
    fn alloc_pod() {
        let alloc = LinearAllocator::new(1024);
        let scratch = ScopedScratch::new(&alloc);

        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        struct A {
            data: u32,
        }

        let a = scratch.new_pod(A {
            data: 0xDEADC0DEu32,
        });
        assert_eq!(a.data, 0xDEADC0DEu32);
    }

    #[test]
    fn alloc_obj() {
        let alloc = LinearAllocator::new(1024);
        let scratch = ScopedScratch::new(&alloc);

        #[allow(dead_code)]
        struct A {
            data: Vec<u32>,
        }

        let a = scratch.new_obj(A {
            data: vec![0xC0FFEEEEu32],
        });
        assert_eq!(a.data.len(), 1);
        assert_eq!(a.data[0], 0xC0FFEEEEu32);
    }

    #[test]
    fn scope_rewind() {
        let alloc = LinearAllocator::new(1024);
        let start_ptr = alloc.peek();
        {
            let scratch = ScopedScratch::new(&alloc);
            let _ = scratch.new_pod(0u32);
            assert_ne!(start_ptr, alloc.peek());
        }
        assert_eq!(start_ptr, alloc.peek());
    }

    #[test]
    fn new_scope() {
        let alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopedScratch::new(&alloc);
            let a = scratch.new_pod(0xCAFEBABEu32);
            assert_eq!(*a, 0xCAFEBABEu32);
            {
                let scratch2 = scratch.new_scope();
                let b = scratch2.new_pod(0xDEADCAFEu32);
                assert_eq!(*b, 0xDEADCAFEu32);
            }
            assert_eq!(*a, 0xCAFEBABEu32);
            let b = scratch.new_pod(0xC0FFEEEEu32);
            assert_eq!(*b, 0xC0FFEEEEu32);
        }
    }

    #[should_panic(
        expected = "Tried to allocate from a ScopedScratch that has an active child scope"
    )]
    #[test]
    fn active_parent_new_pod() {
        let alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopedScratch::new(&alloc);
            let _ = scratch.new_pod(0xCAFEBABEu32);
            {
                let _scratch2 = scratch.new_scope();
                let _ = scratch.new_pod(0xDEADCAFEu32);
            }
        }
    }

    #[test]
    fn dtor_order() {
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

        let alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopedScratch::new(&alloc);

            let _ = scratch.new_obj(A {
                data: 0xCAFEBABEu32,
                dtor_push: &mut dtor_push,
            });
            let _ = scratch.new_obj(A {
                data: 0xDEADCAFEu32,
                dtor_push: &mut dtor_push,
            });
        }
        assert_eq!(dtor_data.len(), 2);
        assert_eq!(dtor_data[0], 0xDEADCAFEu32);
        assert_eq!(dtor_data[1], 0xCAFEBABEu32);
    }
}