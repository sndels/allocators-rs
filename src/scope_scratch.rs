use crate::allocator::{AllocationError, AllocatorInternal, LinearAllocator};

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
    pub fn new_obj<T>(&self, obj: T) -> Result<&mut T, AllocationError> {
        let mut data = self.allocator.alloc_internal(ScopeData {
            mem: std::ptr::null_mut::<u8>(),
            dtor: Some(&|ptr: *mut u8| unsafe { (ptr as *mut T).drop_in_place() }),
            previous: self.data_chain.get(),
        })?;

        match self.allocator.alloc_internal(obj) {
            Ok(ret) => {
                data.mem = (ret as *mut T) as *mut u8;
                self.data_chain.replace(Some(data));
                Ok(ret)
            }
            Err(why) => Err(why), // This leaves the memory allocated for dtor hanging around
        }
    }

    // Safety bounds on allocation, approximate true PoD
    //       Copy - won't have Drop, won't have Boxes.
    //       (could be unhelpful if objects are large but we likely only want to use this for small objects)
    //       Sized - Can be in stack, see above
    //       Send + Sync - No Cells, Rcs
    // TODO: Could this be abstracted such that we could call one method for both
    //       and let the compiler do magic to figure out which it is? Sounds like specialization but for param type.
    pub fn new_pod<T: Copy + Sized + Send + Sync>(
        &self,
        pod: T,
    ) -> Result<&mut T, AllocationError> {
        self.allocator.alloc_internal(pod)
    }
}
