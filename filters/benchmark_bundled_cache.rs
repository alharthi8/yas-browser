//! Explicit work-only CPU probe of the complete production loader. No browser.
#![allow(unexpected_cfgs)]
use adblock::Engine;
use serde_json::json;
use std::{hint::black_box, time::Instant};

#[path = "../src/filter_cache.rs"]
mod filter_cache;
#[cfg(filter_cache_alloc)]
mod allocation {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };
    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
        static COUNTS: Cell<(u64,u64)> = const { Cell::new((0,0)) };
    }
    struct Tracked;
    #[global_allocator]
    static ALLOCATOR: Tracked = Tracked;
    fn record(size: usize) {
        if ENABLED.try_with(Cell::get).unwrap_or(false) {
            COUNTS.with(|counter| {
                let (calls, bytes) = counter.get();
                counter.set((calls + 1, bytes + size as u64));
            });
        }
    }
    unsafe impl GlobalAlloc for Tracked {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = System.alloc(layout);
            if !p.is_null() {
                record(layout.size())
            }
            p
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = System.alloc_zeroed(layout);
            if !p.is_null() {
                record(layout.size())
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            System.dealloc(p, layout)
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let next = System.realloc(p, layout, size);
            if !next.is_null() {
                record(size)
            }
            next
        }
    }
    pub fn start() {
        COUNTS.with(|c| c.set((0, 0)));
        ENABLED.with(|v| v.set(true))
    }
    pub fn stop() -> (u64, u64) {
        ENABLED.with(|v| v.set(false));
        COUNTS.with(Cell::get)
    }
}

fn input() -> String {
    let source = include_str!("../src/blocker.rs");
    let encoded = source
        .lines()
        .find_map(|line| line.trim().strip_prefix("const FALLBACK: &str = "))
        .unwrap();
    let mut input: String = serde_json::from_str(encoded.trim_end_matches(';')).unwrap();
    input.push_str(include_str!("easylist.txt"));
    input.push('\n');
    input.push_str(include_str!("easyprivacy.txt"));
    input.push('\n');
    input
}

fn construct(cached: bool, input: String) -> Engine {
    if cached {
        let (engine, used) = filter_cache::build(black_box(input));
        assert!(
            used,
            "Complete production loader fell back; this cannot be scored as a cache sample"
        );
        engine
    } else {
        Engine::new_with_list_text(black_box(input))
    }
}

fn main() {
    let input = input();
    if std::env::args().nth(1).as_deref() == Some("probe") {
        let (_, used) = filter_cache::build(input);
        println!(
            "{}",
            json!({"actualCompiledBuildStampAccepted": used, "timed": false})
        );
        assert!(
            used,
            "Actual compiled build stamp did not select the immutable cache"
        );
        return;
    }
    assert_eq!(
        std::env::args().nth(1).as_deref(),
        Some(if cfg!(filter_cache_alloc) {
            "allocations"
        } else {
            "timing"
        })
    );
    if cfg!(filter_cache_alloc) {
        #[cfg(filter_cache_alloc)]
        for round in 0..2 {
            for cached in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let prepared = input.clone();
                allocation::start();
                let engine = construct(cached, prepared);
                let (calls, bytes) = allocation::stop();
                println!(
                    "{}",
                    json!({"event":"allocations","round":round+1,"cached":cached,"allocationCalls":calls,"allocatedBytes":bytes,"timingClaims":false})
                );
                drop(black_box(engine));
            }
        }
    } else {
        for cached in [false, true] {
            drop(black_box(construct(cached, input.clone())))
        }
        for round in 0..8 {
            for (position, cached) in (if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            })
            .into_iter()
            .enumerate()
            {
                let mut ns = 0u128;
                let mut individual = Vec::new();
                for _ in 0..3 {
                    let prepared = input.clone();
                    let start = Instant::now();
                    let engine = construct(cached, prepared);
                    let elapsed = start.elapsed();
                    ns += elapsed.as_nanos();
                    individual.push(elapsed.as_nanos());
                    drop(black_box(engine));
                }
                println!(
                    "{}",
                    json!({"event":"timing","round":round+1,"position":position+1,"cached":cached,"iterations":3,"nanosecondsPerConstruction":ns as f64/3.,"individualNanoseconds":individual,"allocationInstrumentation":false,"includesAllProductionHashesAndBuildGates":true})
                );
            }
        }
    }
}
