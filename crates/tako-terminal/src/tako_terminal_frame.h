#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
#define TAKO_TERMINAL_STATIC_ASSERT static_assert
#else
#define TAKO_TERMINAL_STATIC_ASSERT _Static_assert
#endif

// Shared frame-plan data for the private C++<->Zig terminal ABI.
//
// Zig produces this public frame shape for the C++ renderer.

typedef struct Vertex {
    float x;
    float y;
    float u;
    float v;
    uint8_t r;
    uint8_t g;
    uint8_t b;
    uint8_t a;
} Vertex;

TAKO_TERMINAL_STATIC_ASSERT(sizeof(Vertex) == 20, "Vertex ABI layout changed");
TAKO_TERMINAL_STATIC_ASSERT(offsetof(Vertex, r) == 16, "Vertex color offset changed");

typedef struct FramePlan {
    uint8_t clear_color[4];
    float cell_w;
    float cell_h;
    uint32_t cols;
    uint32_t rows;
    uint32_t cursor_x;
    uint32_t cursor_y;
    uint8_t cursor_present;
    const Vertex *vertices;
    uintptr_t vertex_count;
    uint32_t atlas_w;
    uint32_t atlas_h;
    const uint8_t *atlas_pixels;
    uint64_t atlas_generation;
} FramePlan;

TAKO_TERMINAL_STATIC_ASSERT(sizeof(FramePlan) == 72, "FramePlan ABI layout changed");
TAKO_TERMINAL_STATIC_ASSERT(offsetof(FramePlan, vertices) == 32, "FramePlan vertices offset changed");
TAKO_TERMINAL_STATIC_ASSERT(offsetof(FramePlan, atlas_pixels) == 56, "FramePlan atlas offset changed");

#undef TAKO_TERMINAL_STATIC_ASSERT
