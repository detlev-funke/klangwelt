/* Klangwelt: kleinste math.h für libopus auf wasm32-unknown-unknown.
   Wurzel, Betrag, Runden sind WebAssembly-Befehle; sin, cos, exp, log, log10
   und pow liefert Rusts compiler_builtins. */
#ifndef KLANG_WASM_MATH_H
#define KLANG_WASM_MATH_H

#define M_PI 3.14159265358979323846
#define HUGE_VAL __builtin_huge_val()
#define INFINITY __builtin_inff()
#define NAN __builtin_nanf("")

static inline double sqrt(double x) { return __builtin_sqrt(x); }
static inline float sqrtf(float x) { return __builtin_sqrtf(x); }
static inline double fabs(double x) { return __builtin_fabs(x); }
static inline float fabsf(float x) { return __builtin_fabsf(x); }
static inline double floor(double x) { return __builtin_floor(x); }
static inline float floorf(float x) { return __builtin_floorf(x); }
static inline double ceil(double x) { return __builtin_ceil(x); }
static inline float ceilf(float x) { return __builtin_ceilf(x); }
static inline double trunc(double x) { return __builtin_trunc(x); }
static inline double rint(double x) { return __builtin_rint(x); }
static inline float rintf(float x) { return __builtin_rintf(x); }
static inline long lrint(double x) { return (long)__builtin_rint(x); }
static inline long lrintf(float x) { return (long)__builtin_rintf(x); }

double sin(double x);
double cos(double x);
double exp(double x);
double log(double x);
double log10(double x);
double pow(double x, double y);
float sinf(float x);
float cosf(float x);
float expf(float x);
float logf(float x);
float powf(float x, float y);

#endif
