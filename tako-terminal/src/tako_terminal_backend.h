#pragma once

#include <stdbool.h>
#include <stdint.h>

// Private implementation structs used by the Zig terminal core for font,
// snapshot, and frame-planning helpers. The C++ facade must not include this
// header; public terminal/frame ABI lives in tako_terminal_core.h and
// tako_terminal_frame.h.

typedef struct TakoTerminalSurfaceOptions {
    const char *font_path;
    uint32_t pixel_height;
    float dpr;
} TakoTerminalSurfaceOptions;

typedef struct TakoTerminalCellMetrics {
    uint32_t cell_width;
    uint32_t cell_height;
    int32_t cell_ascent;
    int32_t cell_descent;
} TakoTerminalCellMetrics;

typedef struct TakoTerminalRgb {
    uint8_t r;
    uint8_t g;
    uint8_t b;
} TakoTerminalRgb;

typedef struct TakoTerminalColors {
    TakoTerminalRgb foreground;
    TakoTerminalRgb background;
    bool cursor_present;
    TakoTerminalRgb cursor;
    TakoTerminalRgb palette[256];
} TakoTerminalColors;

typedef struct TakoTerminalCellStyle {
    bool bold;
    bool italic;
    bool faint;
    bool blink;
    bool inverse;
    bool invisible;
    bool strikethrough;
    bool overline;
    bool underline;
} TakoTerminalCellStyle;

typedef struct TakoTerminalCell {
    const uint8_t *grapheme_ptr;
    uintptr_t grapheme_len;
    TakoTerminalCellStyle style;
    bool text_visible;
    TakoTerminalRgb text_fg;
    bool fg_present;
    TakoTerminalRgb fg;
    bool bg_present;
    TakoTerminalRgb bg;
} TakoTerminalCell;

typedef struct TakoTerminalRow {
    bool dirty;
    bool selection_present;
    uint16_t selection_start_x;
    uint16_t selection_end_x;
    const TakoTerminalCell *cells;
    uintptr_t cell_count;
} TakoTerminalRow;

typedef struct TakoTerminalFrameSnapshot {
    uint16_t cols;
    uint16_t rows;
    uint32_t dirty;
    TakoTerminalColors colors;
    const TakoTerminalRow *rows_ptr;
    uintptr_t row_count;
} TakoTerminalFrameSnapshot;

typedef struct TakoTerminalShapedGlyph {
    uint32_t glyph_id;
    float x_offset;
    float y_offset;
    float x_advance;
} TakoTerminalShapedGlyph;

typedef struct TakoTerminalShapedText {
    const TakoTerminalShapedGlyph *glyphs;
    uintptr_t glyph_count;
} TakoTerminalShapedText;

typedef struct TakoTerminalRasterizedGlyph {
    uint32_t glyph_id;
    uint32_t width;
    uint32_t height;
    int32_t left_bearing;
    int32_t top_bearing;
    const uint8_t *pixels;
    uintptr_t pixel_len;
} TakoTerminalRasterizedGlyph;
