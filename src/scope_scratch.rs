use crate::allocator::{AllocatorInternal, LinearAllocator};

use std::cell::Cell;

// Inspired by Frostbite's Scope Stack Allocation

struct ScopeData<'a> {
    mem: *mut u8,
    dtor: Option<&'a dyn Fn(*mut u8)>,
    previous: Option<&'a ScopeData<'a>>,
}

pub struct ScopeScratch<'a> {
    allocator: &'a LinearAllocator,
    alloc_start: *mut u8,
    data_chain: Cell<Option<&'a ScopeData<'a>>>,
}

impl Drop for ScopeScratch<'_> {
    fn drop(&mut self) {
        println!("ScopeScratch::drop()");

        let mut data_chain = self.data_chain.get();
        while let Some(scope) = data_chain {
            if let Some(dtor) = scope.dtor {
                dtor(scope.mem)
            }
            data_chain = scope.previous;
        }

        unsafe {
            self.allocator.rewind(self.alloc_start);
        }
    }
}

impl<'a> ScopeScratch<'a> {
    pub fn new(allocator: &'a LinearAllocator) -> Self {
        Self {
            allocator,
            alloc_start: allocator.peek(),
            data_chain: Cell::new(None),
        }
    }

    pub fn new_scope(&self) -> Self {
        Self::new(self.allocator)
    }

    // TODO: Can we get away with no Drop?
    //       Aggregate can have no Drop of its own but store data that implements it.
    //       How does drop_in_place behave then?
    pub fn new_obj<T>(&self, obj: T) -> &mut T {
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

    // Safety bounds on allocation, approximate true PoD
    //       Copy - won't have Drop, won't have Boxes.
    //       (could be unhelpful if objects are large but we likely only want to use this for small objects)
    //       Sized - Can be in stack, see above
    //       Send + Sync - No Cells, Rcs
    // TODO: Could this be abstracted such that we could call one method for both
    //       and let the compiler do magic to figure out which it is? Sounds like specialization but for param type.
    pub fn new_pod<T: Copy + Sized + Send + Sync>(&self, pod: T) -> &mut T {
        self.allocator.alloc_internal(pod)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn alloc_primitive() {
        let alloc = LinearAllocator::new(1024);
        let scratch = ScopeScratch::new(&alloc);

        let a = scratch.new_pod(0xABu8);
        assert_eq!(*a, 0xABu8);
    }

    #[test]
    fn alloc_pod() {
        let alloc = LinearAllocator::new(1024);
        let scratch = ScopeScratch::new(&alloc);

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
        let scratch = ScopeScratch::new(&alloc);

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
            let scratch = ScopeScratch::new(&alloc);
            let _ = scratch.new_pod(0u32);
            assert_ne!(start_ptr, alloc.peek());
        }
        assert_eq!(start_ptr, alloc.peek());
    }

    #[test]
    fn new_scope() {
        let alloc = LinearAllocator::new(1024);
        {
            let scratch = ScopeScratch::new(&alloc);
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
            let scratch = ScopeScratch::new(&alloc);

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
