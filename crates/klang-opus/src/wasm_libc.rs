//! Memory for libopus in the browser (wasm32-unknown-unknown has no libc):
//! `malloc` & co. via Rust's allocator. Math functions and `memcpy` & co.
//! are already provided by Rust's compiler_builtins; libopus needs nothing more
//! (build-web checks that klang_web.wasm imports nothing from outside).

use std::alloc::{alloc, alloc_zeroed, dealloc, realloc as grow, Layout};

/// Each block is preceded by its size; 16 bytes keep the alignment right for everything.
const HEAD: usize = 16;

fn layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(HEAD)?, HEAD).ok()
}

unsafe fn finish(base: *mut u8, size: usize) -> *mut u8 {
    if base.is_null() {
        return base;
    }
    (base as *mut usize).write(size);
    base.add(HEAD)
}

#[no_mangle]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    match layout(size) {
        Some(l) => finish(alloc(l), size),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn calloc(n: usize, size: usize) -> *mut u8 {
    match n.checked_mul(size).and_then(|s| Some((s, layout(s)?))) {
        Some((s, l)) => finish(alloc_zeroed(l), s),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    if ptr.is_null() {
        return malloc(size);
    }
    let base = ptr.sub(HEAD);
    let old = (base as *const usize).read();
    match (layout(old), layout(size)) {
        (Some(l), Some(_)) => finish(grow(base, l, size + HEAD), size),
        _ => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let base = ptr.sub(HEAD);
    let size = (base as *const usize).read();
    if let Some(l) = layout(size) {
        dealloc(base, l);
    }
}
