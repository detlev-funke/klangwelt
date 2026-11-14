/* Klangwelt: kleinste string.h für libopus auf wasm32-unknown-unknown.
   memcpy & Co. liefert Rusts compiler_builtins. */
#ifndef KLANG_WASM_STRING_H
#define KLANG_WASM_STRING_H
#include <stddef.h>

void *memcpy(void *dst, const void *src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
void *memset(void *dst, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);

#endif
