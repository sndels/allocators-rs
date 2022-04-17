use allocators::{LinearAllocator, ScopeScratch};

use std::time::Instant;

trait BenchNew {
    fn new(v: u32) -> Self;
}
trait BenchData {
    fn data(&self, i: usize) -> u32;
}

impl<T: BenchData> BenchData for Box<T> {
    fn data(&self, i: usize) -> u32 {
        (**self).data(i)
    }
}

impl<T: BenchData> BenchData for &mut T {
    fn data(&self, i: usize) -> u32 {
        (**self).data(i)
    }
}

macro_rules! declare_structs {
    ($pod_name:ident, $obj_name:ident, $size:literal) => {
        #[derive(Copy, Clone, Debug)]
        struct $pod_name {
            data: [u32; $size / 4],
        }

        impl BenchNew for $pod_name {
            fn new(v: u32) -> Self {
                Self {
                    data: [v; $size / 4],
                }
            }
        }

        impl BenchData for $pod_name {
            fn data(&self, i: usize) -> u32 {
                self.data[i]
            }
        }

        struct $obj_name {
            data: [u32; $size / 4],
        }

        impl BenchNew for $obj_name {
            fn new(v: u32) -> Self {
                Self {
                    data: [v; $size / 4],
                }
            }
        }

        impl BenchData for $obj_name {
            fn data(&self, i: usize) -> u32 {
                self.data[i]
            }
        }

        impl Drop for $obj_name {
            fn drop(&mut self) {
                ()
            }
        }
    };
}

declare_structs!(CacheLine64, ObjCacheLine64, 64);
declare_structs!(CacheLine128, ObjCacheLine128, 128);
declare_structs!(CacheLine256, ObjCacheLine256, 256);
declare_structs!(CacheLine512, ObjCacheLine512, 512);
declare_structs!(CacheLine1k, ObjCacheLine1k, 1024);

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
    scoped_pod: Timing,
    scoped_obj: Timing,
}

impl Default for TestTimes {
    fn default() -> Self {
        Self {
            naive_pod: Timing::default(),
            naive_obj: Timing::default(),
            scoped_pod: Timing::default(),
            scoped_obj: Timing::default(),
        }
    }
}
const ITEM_COUNT: usize = 2_000_000;
const ITERATIONS: usize = 10;
const TOTAL_ALLOCATIONS: usize = ITEM_COUNT * ITERATIONS;

fn bench_alloc<'a, T: BenchData>(
    scratch: &'a ScopeScratch,
    alloc: &dyn Fn(&'a ScopeScratch, u32) -> T,
) -> (Vec<T>, f32) {
    let start = Instant::now();
    let datas: Vec<T> = (0..ITEM_COUNT as u32).map(|v| alloc(scratch, v)).collect();
    let end = Instant::now();
    let spent_ns = (end - start).as_nanos() as f32;
    (datas, spent_ns)
}

fn bench_iter<T: BenchData>(datas: &Vec<T>) -> (u32, f32) {
    let start = Instant::now();
    let mut v = 0;
    let mut acc = 0u32;
    for d in datas {
        acc = acc.wrapping_add(d.data(v));
        v = (v + 1) % 16;
    }
    let end = Instant::now();
    let spent_ns = (end - start).as_nanos() as f32;
    (acc, spent_ns)
}

fn new_pod<'a, T: Copy + BenchNew + BenchData>(scratch: &'a ScopeScratch, v: u32) -> &'a mut T {
    scratch.new_pod(T::new(v))
}

fn new_obj<'a, T: BenchNew + BenchData>(scratch: &'a ScopeScratch, v: u32) -> &'a mut T {
    scratch.new_obj(T::new(v))
}

fn bench<T: Copy + BenchNew + BenchData, V: BenchNew + BenchData>() -> String {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<V>());

    println!(
        "{} and {}",
        std::any::type_name::<T>(),
        std::any::type_name::<V>()
    );

    let mut times = TestTimes::default();

    // Allocate space for both the objects and potential ScopeData
    let allocator = Box::new(LinearAllocator::new(
        ITEM_COUNT * (std::mem::size_of::<T>() + 32),
    ));

    macro_rules! bench {
        ($name:expr, $time:expr, $alloc_fn:expr) => {
            let mut tot_acc = 0u32;
            for i in 0..ITERATIONS {
                println!("{} iter {}", $name, i);
                let dtor_start = {
                    let scope = ScopeScratch::new(&allocator);
                    let (datas, alloc_ns) = bench_alloc(&scope, $alloc_fn);
                    $time.alloc_ns += alloc_ns;
                    let (acc, iter_ns) = bench_iter(&datas);
                    tot_acc = tot_acc.wrapping_add(acc);
                    $time.iter_ns += iter_ns;
                    Instant::now()
                };
                let dtor_end = Instant::now();
                $time.dtor_ns += (dtor_end - dtor_start).as_nanos() as f32;
            }
            println!("{}", tot_acc);
            $time.alloc_ns /= TOTAL_ALLOCATIONS as f32;
            $time.iter_ns /= TOTAL_ALLOCATIONS as f32;
            $time.dtor_ns /= TOTAL_ALLOCATIONS as f32;
        };
    }

    bench!("Naive POD", times.naive_pod, &|_, v| Box::new(T::new(v)));

    bench!("Naive obj", times.naive_obj, &|_, v| Box::new(V::new(v)));

    bench!("Scoped POD", times.scoped_pod, &new_pod::<T>);

    bench!("Scoped obj", times.scoped_obj, &new_obj::<V>);

    macro_rules! alloc_diff {
        ($this:ident, $other:ident) => {
            (times.$this.alloc_ns / times.$other.alloc_ns * 100.0) as u32
        };
    }

    macro_rules! iter_diff {
        ($this:ident, $other:ident) => {
            (times.$this.iter_ns / times.$other.iter_ns * 100.0) as u32
        };
    }
    macro_rules! dtor_diff {
        ($this:ident, $other:ident) => {
            (times.$this.dtor_ns / times.$other.dtor_ns * 100.0) as u32
        };
    }

    // NOTE: Iter times are really close between the naive versions and linear allocator.
    //       Seems like repeated box allocations are done linearly, but are they optimized to
    //       a single large allocation or do we just get lucky with the tight loop getting
    //       contiguous addresses?
    let mut ret = String::new();
    ret += &format!("Results (average per item)\n");
    ret += &format!("Struct size: {}\n", std::mem::size_of::<T>());
    ret += &format!("  Naive POD boxing\n");
    ret += &format!("    Alloc {:.2}ns\n", times.naive_pod.alloc_ns);
    ret += &format!("    Iter {:.2}ns\n", times.naive_pod.iter_ns);
    ret += &format!("    Dtor {:.2}ns\n", times.naive_pod.dtor_ns);
    ret += &format!("  Naive obj boxing\n");
    ret += &format!(
        "    Alloc {:.2}ns ({}% of naive POD)\n",
        times.naive_obj.alloc_ns,
        alloc_diff!(naive_obj, naive_pod)
    );
    ret += &format!(
        "    Iter {:.2}ns ({}% of naive POD)\n",
        times.naive_obj.iter_ns,
        iter_diff!(naive_obj, naive_pod)
    );
    ret += &format!(
        "    Dtor {:.2}ns ({}% of naive POD)\n",
        times.naive_obj.dtor_ns,
        dtor_diff!(naive_obj, naive_pod)
    );
    ret += &format!("  Scoped POD\n");
    ret += &format!(
        "    Alloc {:.2}ns ({}% of naive POD)\n",
        times.scoped_pod.alloc_ns,
        alloc_diff!(scoped_pod, naive_pod)
    );
    ret += &format!(
        "    Iter {:.2}ns ({}% of naive POD)\n",
        times.scoped_pod.iter_ns,
        iter_diff!(scoped_pod, naive_pod)
    );
    ret += &format!(
        "    Dtor {:.2}ns ({}% of naive POD)\n",
        times.scoped_pod.dtor_ns,
        dtor_diff!(scoped_pod, naive_pod)
    );
    ret += &format!("  Scoped obj\n");
    ret += &format!(
        "    Alloc {:.2}ns ({}% of naive POD, {}% of scoped POD, {}% of naive obj)\n",
        times.scoped_obj.alloc_ns,
        alloc_diff!(scoped_obj, naive_pod),
        alloc_diff!(scoped_obj, scoped_pod),
        alloc_diff!(scoped_obj, naive_obj)
    );
    ret += &format!(
        "    Iter {:.2}ns ({}% of naive POD, {}% of scoped POD, {}% of naive obj)\n",
        times.scoped_obj.iter_ns,
        iter_diff!(scoped_obj, naive_pod),
        iter_diff!(scoped_obj, scoped_pod),
        iter_diff!(scoped_obj, naive_obj)
    );
    ret += &format!(
        "    Dtor {:.2}ns ({}% of naive POD, {}% of scoped POD, {}% of naive obj)\n",
        times.scoped_obj.dtor_ns,
        dtor_diff!(scoped_obj, naive_pod),
        dtor_diff!(scoped_obj, scoped_pod),
        dtor_diff!(scoped_obj, naive_obj)
    );
    ret
}

fn main() {
    let mut results = vec![];
    results.push(bench::<CacheLine64, ObjCacheLine64>());
    results.push(bench::<CacheLine128, ObjCacheLine128>());
    results.push(bench::<CacheLine256, ObjCacheLine256>());
    results.push(bench::<CacheLine512, ObjCacheLine512>());
    results.push(bench::<CacheLine1k, ObjCacheLine1k>());
    println!("{}", results.join("\n"));
}
