/* Klangwelt: kleinste stdlib.h für libopus auf wasm32-unknown-unknown (ohne libc).
   Speicher kommt aus Rust (src/wasm_libc.rs). */
#ifndef KLANG_WASM_STDLIB_H
#define KLANG_WASM_STDLIB_H
#include <stddef.h>

void *malloc(size_t size);
void *calloc(size_t n, size_t size);
void *realloc(void *ptr, size_t size);
void free(void *ptr);

static inline int abs(int x) { return x < 0 ? -x : x; }
static inline long labs(long x) { return x < 0 ? -x : x; }
#define abort() __builtin_trap()

#endif
