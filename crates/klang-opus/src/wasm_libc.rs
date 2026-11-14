//! Speicher für libopus im Browser (wasm32-unknown-unknown hat keine libc):
//! `malloc` & Co. über Rusts Allokator. Mathe-Funktionen und `memcpy` & Co.
//! bringt Rusts compiler_builtins schon mit; mehr braucht libopus nicht
//! (build-web prüft, dass klang_web.wasm nichts von außen importiert).

use std::alloc::{alloc, alloc_zeroed, dealloc, realloc as grow, Layout};

/// Vor jedem Block steht seine Größe; 16 Byte halten die Ausrichtung für alles.
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
