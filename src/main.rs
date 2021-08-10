mod allocator;
mod scope_scratch;

use allocator::{AllocatorInternal, LinearAllocator};
use scope_scratch::ScopeScratch;

#[derive(Copy, Clone, Debug)]
struct Vec3<T> {
    x: T,
    y: T,
    z: T,
}

#[derive(Debug)]
struct A {
    dummy: u8,
}
struct B {
    dummy: u128,
}
struct C {
    dummy: u32,
}

impl Drop for A {
    fn drop(&mut self) {
        println!("Drop A");
    }
}
impl Drop for B {
    fn drop(&mut self) {
        println!("Drop B");
    }
}
impl Drop for C {
    fn drop(&mut self) {
        println!("Drop C");
    }
}

fn main() {
    let allocator = LinearAllocator::new(512).unwrap();
    let global_scope = ScopeScratch::new(&allocator);
    let a = global_scope.new_pod(-1.0).unwrap();
    let b = global_scope.new_pod(1).unwrap();
    println!("a {} b {}", a, b);
    *a += 1.0;
    *b += 2;
    println!("a {} b {}", a, b);
    let c = {
        global_scope
            .new_pod(Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            })
            .unwrap()
    };
    let d = {
        global_scope
            .new_obj(Vec3 {
                x: A { dummy: 0xAB },
                y: A { dummy: 0xCD },
                z: A { dummy: 0xDF },
            })
            .unwrap()
    };
    println!("a {} b {} c {:?} d {:?}", a, b, c, d);

    println!("peek {:?}", allocator.peek());
    {
        let scope = global_scope.new_scope();
        let a = scope.new_obj(A { dummy: 0xFF }).unwrap();
        let b = scope
            .new_obj(B {
                dummy: 0xFFFFFFFFFFFFFF,
            })
            .unwrap();
        let c = scope.new_obj(C { dummy: 0xFFFFFFFF }).unwrap();
        let d = scope.new_pod(Vec3 { x: 0, y: 1, z: 2 }).unwrap();
        println!("a {} b {} c {} d {:?}", a.dummy, b.dummy, c.dummy, d);
        println!("peek {:?}", allocator.peek());
        {
            let scope = scope.new_scope();
            let a = scope.new_obj(A { dummy: 0xAA }).unwrap();
            let b = scope
                .new_obj(B {
                    dummy: 0xAAAAAAAAAAAA,
                })
                .unwrap();
            let c = scope.new_obj(C { dummy: 0xAAAAAAAA }).unwrap();
            let d = scope.new_pod(Vec3 { x: 3, y: 4, z: 5 }).unwrap();
            println!("a {} b {} c {} d {:?}", a.dummy, b.dummy, c.dummy, d);
            println!("peek {:?}", allocator.peek());
        }
    }
    println!("peek {:?}", allocator.peek());
}
