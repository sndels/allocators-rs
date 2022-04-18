# allocators-rs

## What?

An implementation of Frostbite's [Scope Stack Allocation](https://www.ea.com/frostbite/news/scope-stack-allocation) in unsafe Rust. Tries to create a safe interface with a combination of proper lifetimes, borrow rules, and runtime asserts for interior mutability.

`ScopedScratch` is an allocator that can allocate both POD types (types that implement `Copy`) and types that have to be dropped. The latter is supported internally by storing a chain of destructors that is called in reverse allocation order when the scratch is dropped.

`ScopedScratch` is backed by a simple linear allocator that holds a single, non-resizeable block of heap allocated memory. The allocated objects share the lifetime of the scratch they are allocated from. Scopes can also create child scopes backed by the same allocator, and runtime asserts enforce that only the innermost scope is ever allocated from. Surprisingly, the performance impact of this runtime checking seems negligible even with allocations that only span a single cache line.


```rust
#[derive(Clone, Copy)]
struct Pod {
    data: [u32; 16],
}

struct Object {
    data: Vec<u32>,
}

fn main() {
    let mut allocator = LinearAllocator::new(1024);
    let prim: &mut u32;
    {
        let scratch = ScopedScratch::new(&mut allocator);
        let obj: &mut Object = scratch.alloc(Object { data: vec![0; 16] });
        let pod: &mut Pod = scratch.alloc(Pod { data: [0; 16] });
        prim = scratch.alloc(0u32);
        {
            let inner_scratch = scratch.new_scope();
            let inner_pod: &mut Pod = inner_scratch.alloc(Pod { data: [0; 16] });
            // This will panic on assert since `scratch` is the parent of `inner_scratch`
            // let scratch_pod: &mut Pod = scratch.alloc(Pod { data: [0; 16] });
        }
        // The object `obj` points to will be dropped here
    }
    // This will not compile since `scratch` doesn't live long enough
    // *prim = 1;
}
```

## Why?

Unsafe Rust is something I had wanted to toy with for a while and this kind of allocation can be very beneficial in some contexts, so it seemed like a good exercise.

The benchmark included in the repo verifies that the implementation is indeed faster per object than individual `Box` allocations for both both POD types and types that (might) have a dtor (Ryzen 5900X, Windows 10). It runs ten iterations of allocating (and initializing) 2 million objects, iterating a sum over them and timing the drops of said allocations when they go out of scope.

```
Struct size: 64
  Naive POD boxing
    Alloc 39.28ns
    Iter 5.36ns
    Dtor 36.62ns
  Naive obj boxing
    Alloc 38.57ns (98% of naive POD)
    Iter 5.08ns (94% of naive POD)
    Dtor 35.38ns (96% of naive POD)
  Scoped POD
    Alloc 5.96ns (15% of naive POD)
    Iter 2.45ns (45% of naive POD)
    Dtor 0.16ns (0% of naive POD)
  Scoped obj
    Alloc 10.11ns (25% of naive POD, 169% of scoped POD, 26% of naive obj)
    Iter 4.01ns (74% of naive POD, 163% of scoped POD, 79% of naive obj)
    Dtor 4.86ns (13% of naive POD, 3022% of scoped POD, 13% of naive obj)


Struct size: 1024
  Naive POD boxing
    Alloc 313.69ns
    Iter 6.90ns
    Dtor 129.25ns
  Naive obj boxing
    Alloc 320.28ns (102% of naive POD)
    Iter 7.14ns (103% of naive POD)
    Dtor 131.13ns (101% of naive POD)
  Scoped POD
    Alloc 79.16ns (25% of naive POD)
    Iter 5.47ns (79% of naive POD)
    Dtor 0.15ns (0% of naive POD)
  Scoped obj
    Alloc 82.22ns (26% of naive POD, 103% of scoped POD, 25% of naive obj)
    Iter 5.31ns (76% of naive POD, 96% of scoped POD, 74% of naive obj)
    Dtor 6.62ns (5% of naive POD, 4423% of scoped POD, 5% of naive obj)

```
There is some additional overhead from using a `Vec` to store the individual objects in the benchmark, but said overhead should be the same for all tested implementations and only vary based on the used struct size. I suspect the relative impact of that overhead is the greatest for the scoped POD dtor average since a single scratch dtor with no droppable allocations takes less than 30ns on average even for inner scopes, while this benchmark suggests an average of hundreds of microseconds.

Note that the benchmark does `Box` allocations back-to-back, which likely keeps the individual objects close by in memory. Results might be even worse for the `Box` allocations if there were other allocations, or more time, in between individual object allocations. That could be an interesting benchmark case :)
