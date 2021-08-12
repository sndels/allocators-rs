mod allocator;
mod scope_scratch;

use allocator::{AllocatorInternal, LinearAllocator};
use scope_scratch::ScopeScratch;

use std::time::Instant;

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

#[derive(Copy, Clone, Debug)]
struct CacheLine {
    data: [u32; 16],
}

impl CacheLine {
    pub fn new(v: u32) -> Self {
        Self { data: [v; 16] }
    }
}

struct ObjCacheLine {
    data: [u32; 16],
}

impl ObjCacheLine {
    pub fn new(v: u32) -> Self {
        Self { data: [v; 16] }
    }
}

impl Drop for ObjCacheLine {
    fn drop(&mut self) {
        ()
    }
}

struct Timing {
    alloc_ns: f32,
    iter_ns: f32,
    dtor_ns: f32,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            alloc_ns: 0.0,
            iter_ns: 0.0,
            dtor_ns: 0.0,
        }
    }
}

struct TestTimes {
    naive_pod: Timing,
    naive_obj: Timing,
    linear: Timing,
    scoped_pod: Timing,
    scoped_obj: Timing,
}

impl Default for TestTimes {
    fn default() -> Self {
        Self {
            naive_pod: Timing::default(),
            naive_obj: Timing::default(),
            linear: Timing::default(),
            scoped_pod: Timing::default(),
            scoped_obj: Timing::default(),
        }
    }
}
const items: usize = 1024 * 1024;

fn bench_alloc<T>(alloc: &dyn Fn(u32) -> T) -> (Vec<T>, f32) {
    let start = Instant::now();
    let datas: Vec<T> = (0..items as u32).map(|v| alloc(v)).collect();
    let end = Instant::now();
    let spent_ns = (end - start).as_nanos() as f32;
    (datas, spent_ns)
}

fn bench_iter<T>(datas: &Vec<T>, iter: &dyn Fn(&T, usize) -> u32) -> f32 {
    let start = Instant::now();
    let mut v = 0;
    let mut acc = 0u32;
    for d in datas {
        acc += iter(&d, v);
        v = (v + 1) % 16;
    }
    let end = Instant::now();
    println!("Acc {}", acc);
    let spent_ns = (end - start).as_nanos() as f32;
    spent_ns
}

fn main() {
    // let allocator = LinearAllocator::new(512).unwrap();
    // let global_scope = ScopeScratch::new(&allocator);
    // let a = global_scope.new_pod(-1.0).unwrap();
    // let b = global_scope.new_pod(1).unwrap();
    // println!("a {} b {}", a, b);
    // *a += 1.0;
    // *b += 2;
    // println!("a {} b {}", a, b);
    // let c = {
    //     global_scope
    //         .new_pod(Vec3 {
    //             x: 0.0,
    //             y: 0.0,
    //             z: 0.0,
    //         })
    //         .unwrap()
    // };
    // let d = {
    //     global_scope
    //         .new_obj(Vec3 {
    //             x: A { dummy: 0xAB },
    //             y: A { dummy: 0xCD },
    //             z: A { dummy: 0xDF },
    //         })
    //         .unwrap()
    // };
    // println!("a {} b {} c {:?} d {:?}", a, b, c, d);

    // println!("peek {:?}", allocator.peek());
    // {
    //     let scope = global_scope.new_scope();
    //     let a = scope.new_obj(A { dummy: 0xFF }).unwrap();
    //     let b = scope
    //         .new_obj(B {
    //             dummy: 0xFFFFFFFFFFFFFF,
    //         })
    //         .unwrap();
    //     let c = scope.new_obj(C { dummy: 0xFFFFFFFF }).unwrap();
    //     let d = scope.new_pod(Vec3 { x: 0, y: 1, z: 2 }).unwrap();
    //     println!("a {} b {} c {} d {:?}", a.dummy, b.dummy, c.dummy, d);
    //     println!("peek {:?}", allocator.peek());
    //     {
    //         let scope = scope.new_scope();
    //         let a = scope.new_obj(A { dummy: 0xAA }).unwrap();
    //         let b = scope
    //             .new_obj(B {
    //                 dummy: 0xAAAAAAAAAAAA,
    //             })
    //             .unwrap();
    //         let c = scope.new_obj(C { dummy: 0xAAAAAAAA }).unwrap();
    //         let d = scope.new_pod(Vec3 { x: 3, y: 4, z: 5 }).unwrap();
    //         println!("a {} b {} c {} d {:?}", a.dummy, b.dummy, c.dummy, d);
    //         println!("peek {:?}", allocator.peek());
    //     }
    // }
    // println!("peek {:?}", allocator.peek());

    let mut times = TestTimes::default();
    let iterations = 10;
    for _ in 0..iterations {
        let start = {
            let (datas, alloc_ns) = bench_alloc(&|v| Box::new(CacheLine::new(v)));
            times.naive_pod.alloc_ns += alloc_ns;
            times.naive_pod.iter_ns += bench_iter(&datas, &|cache_line, v| cache_line.data[v]);
            Instant::now()
        };
        let end = Instant::now();
        times.naive_pod.dtor_ns += (end - start).as_nanos() as f32;
    }
    for _ in 0..iterations {
        let start = {
            let (datas, alloc_ns) = bench_alloc(&|v| Box::new(ObjCacheLine::new(v)));
            times.naive_obj.alloc_ns += alloc_ns;
            times.naive_obj.iter_ns += bench_iter(&datas, &|cache_line, v| cache_line.data[v]);
            Instant::now()
        };
        let end = Instant::now();
        times.naive_obj.dtor_ns += (end - start).as_nanos() as f32;
    }
    for _ in 0..iterations {
        let start = {
            let allocator = LinearAllocator::new(1024 * 1024 * 512).unwrap();
            let (datas, alloc_ns) =
                bench_alloc(&|v| allocator.alloc_internal(CacheLine::new(v)).unwrap());
            times.linear.alloc_ns += alloc_ns;
            times.linear.iter_ns += bench_iter(&datas, &|cache_line, v| cache_line.data[v]);
            Instant::now()
        };
        let end = Instant::now();
        times.linear.dtor_ns += (end - start).as_nanos() as f32;
    }
    for _ in 0..iterations {
        let start = {
            let allocator = Box::new(LinearAllocator::new(1024 * 1024 * 512).unwrap());
            let scope = ScopeScratch::new(allocator.as_ref());
            let (datas, alloc_ns) = bench_alloc(&|v| scope.new_pod(CacheLine::new(v)).unwrap());
            times.scoped_pod.alloc_ns += alloc_ns;
            times.scoped_pod.iter_ns += bench_iter(&datas, &|cache_line, v| cache_line.data[v]);
            Instant::now()
        };
        let end = Instant::now();
        times.scoped_pod.dtor_ns += (end - start).as_nanos() as f32;
    }
    for _ in 0..iterations {
        let start = {
            let allocator = Box::new(LinearAllocator::new(1024 * 1024 * 512).unwrap());
            let scope = ScopeScratch::new(allocator.as_ref());
            let (datas, alloc_ns) = bench_alloc(&|v| scope.new_obj(ObjCacheLine::new(v)).unwrap());
            times.scoped_obj.alloc_ns += alloc_ns;
            times.scoped_obj.iter_ns += bench_iter(&datas, &|cache_line, v| cache_line.data[v]);
            Instant::now()
        };
        let end = Instant::now();
        times.scoped_obj.dtor_ns += (end - start).as_nanos() as f32;
    }

    times.naive_pod.alloc_ns /= (iterations * items) as f32;
    times.naive_pod.iter_ns /= (iterations * items) as f32;
    times.naive_pod.dtor_ns /= (iterations * items) as f32;
    times.naive_obj.alloc_ns /= (iterations * items) as f32;
    times.naive_obj.iter_ns /= (iterations * items) as f32;
    times.naive_obj.dtor_ns /= (iterations * items) as f32;
    times.linear.alloc_ns /= (iterations * items) as f32;
    times.linear.iter_ns /= (iterations * items) as f32;
    times.linear.dtor_ns /= (iterations * items) as f32;
    times.scoped_pod.alloc_ns /= (iterations * items) as f32;
    times.scoped_pod.iter_ns /= (iterations * items) as f32;
    times.scoped_pod.dtor_ns /= (iterations * items) as f32;
    times.scoped_obj.alloc_ns /= (iterations * items) as f32;
    times.scoped_obj.iter_ns /= (iterations * items) as f32;
    times.scoped_obj.dtor_ns /= (iterations * items) as f32;

    // NOTE: Iter times are really close between the naive versions and linear allocator.
    //       Seems like repeated box allocations are done linearly, but are they optimized to
    //       a single large allocation or do we just get lucky with the tight loop getting
    //       contiguous addresses?
    println!("Results (average per item)");
    println!("  Naive pod boxing");
    println!("    Alloc {:.2}ns", times.naive_pod.alloc_ns);
    println!("    Iter {:.2}ns", times.naive_pod.iter_ns);
    println!("    Dtor {:.2}ns", times.naive_pod.dtor_ns);
    println!("  Naive obj boxing");
    println!(
        "    Alloc {:.2}ns ({}% of naive pod)",
        times.naive_obj.alloc_ns,
        times.naive_obj.alloc_ns / times.naive_pod.alloc_ns * 100.0
    );
    println!(
        "    Iter {:.2}ns ({}% of naive pod)",
        times.naive_obj.iter_ns,
        times.naive_obj.iter_ns / times.naive_pod.iter_ns * 100.0
    );
    println!(
        "    Dtor {:.2}ns ({}% of naive pod)",
        times.naive_obj.dtor_ns,
        times.naive_obj.dtor_ns / times.naive_pod.dtor_ns * 100.0
    );
    println!("  Linear allocator");
    println!(
        "    Alloc {:.2}ns ({}% of naive pod)",
        times.linear.alloc_ns,
        times.linear.alloc_ns / times.naive_pod.alloc_ns * 100.0
    );
    println!(
        "    Iter {:.2}ns ({}% of naive pod)",
        times.linear.iter_ns,
        times.linear.iter_ns / times.naive_pod.iter_ns * 100.0
    );
    println!(
        "    Dtor {:.2}ns ({}% of naive pod)",
        times.linear.dtor_ns,
        times.linear.dtor_ns / times.naive_pod.dtor_ns * 100.0
    );
    println!("  Scoped pod");
    println!(
        "    Alloc {:.2}ns ({}% of naive pod, {}% of linear)",
        times.scoped_pod.alloc_ns,
        times.scoped_pod.alloc_ns / times.naive_pod.alloc_ns * 100.0,
        times.scoped_pod.alloc_ns / times.linear.alloc_ns * 100.0
    );
    println!(
        "    Iter {:.2}ns ({}% of naive pod, {}% of linear)",
        times.scoped_pod.iter_ns,
        times.scoped_pod.iter_ns / times.naive_pod.iter_ns * 100.0,
        times.scoped_pod.iter_ns / times.linear.iter_ns * 100.0
    );
    println!(
        "    Dtor {:.2}ns ({}% of naive pod, {}% of linear)",
        times.scoped_pod.dtor_ns,
        times.scoped_pod.dtor_ns / times.naive_pod.dtor_ns * 100.0,
        times.scoped_pod.dtor_ns / times.linear.dtor_ns * 100.0
    );
    println!("  Scoped obj");
    println!(
        "    Alloc {:.2}ns ({}% of naive pod, {}% of linear, {}% of scoped pod)",
        times.scoped_obj.alloc_ns,
        times.scoped_obj.alloc_ns / times.naive_pod.alloc_ns * 100.0,
        times.scoped_obj.alloc_ns / times.linear.alloc_ns * 100.0,
        times.scoped_obj.alloc_ns / times.scoped_pod.alloc_ns * 100.0
    );
    println!(
        "    Iter {:.2}ns ({}% of naive pod, {}% of linear, {}% of scoped pod)",
        times.scoped_obj.iter_ns,
        times.scoped_obj.iter_ns / times.naive_pod.iter_ns * 100.0,
        times.scoped_obj.iter_ns / times.linear.iter_ns * 100.0,
        times.scoped_obj.iter_ns / times.scoped_pod.iter_ns * 100.0
    );
    println!(
        "    Dtor {:.2}ns ({}% of naive pod, {}% of linear, {}% of scoped pod)",
        times.scoped_obj.dtor_ns,
        times.scoped_obj.dtor_ns / times.naive_pod.dtor_ns * 100.0,
        times.scoped_obj.dtor_ns / times.linear.dtor_ns * 100.0,
        times.scoped_obj.dtor_ns / times.scoped_pod.dtor_ns * 100.0
    );
}
