#pragma once

#ifdef __cplusplus
#include <cstddef>
#include <cstdint>
#else
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#endif

#include "tako_terminal_frame.h"

typedef struct TakoTerminalSession TakoTerminalSession;

typedef struct TakoTerminalOptions {
    uint16_t cols;
    uint16_t rows;
    const char *font_path;
    const char *font_family;
    uint32_t pixel_height;
    float dpr;
    const char *program;
    const char *working_directory;
    uintptr_t max_scrollback;
    bool shell_integration;
} TakoTerminalOptions;

typedef struct TakoTerminalScrollbarState {
    uint64_t total;
    uint64_t offset;
    uint64_t len;
    uint8_t viewport_active;
} TakoTerminalScrollbarState;

typedef struct TakoTerminalBytes {
    const uint8_t *ptr;
    uintptr_t len;
} TakoTerminalBytes;

#ifdef __cplusplus
extern "C" {
#endif

uintptr_t tako_terminal_core_engine_version(uint8_t *out_buf, uintptr_t cap);
void tako_terminal_bytes_free(TakoTerminalBytes bytes);

TakoTerminalSession *tako_terminal_session_new(const TakoTerminalOptions *options);
void tako_terminal_session_destroy(TakoTerminalSession *session);

bool tako_terminal_session_tick(TakoTerminalSession *session, FramePlan *out);
int32_t tako_terminal_session_notify_fd(TakoTerminalSession *session);
int32_t tako_terminal_session_exited(TakoTerminalSession *session);
void tako_terminal_session_drain_notify(TakoTerminalSession *session);

void tako_terminal_session_resize_pixels(TakoTerminalSession *session,
                                         uint32_t width_px,
                                         uint32_t height_px);
void tako_terminal_session_set_dpr(TakoTerminalSession *session, float dpr);
void tako_terminal_session_set_focused(TakoTerminalSession *session, bool focused);
void tako_terminal_session_set_cursor_blink_visible(TakoTerminalSession *session,
                                                    bool visible);
void tako_terminal_session_set_preedit(TakoTerminalSession *session,
                                       const uint8_t *data,
                                       uintptr_t len,
                                       uintptr_t cursor_byte);
int32_t tako_terminal_session_set_default_color(TakoTerminalSession *session,
                                                uint32_t role,
                                                bool enabled,
                                                uint8_t r,
                                                uint8_t g,
                                                uint8_t b);
int32_t tako_terminal_session_set_default_palette(TakoTerminalSession *session,
                                                  bool enabled,
                                                  const uint8_t *rgb_triplets,
                                                  uintptr_t len);
int32_t tako_terminal_session_set_default_cursor(TakoTerminalSession *session,
                                                 uint32_t style,
                                                 bool blink);
int32_t tako_terminal_session_set_font(TakoTerminalSession *session,
                                       const char *font_path,
                                       const char *font_family,
                                       uint32_t pixel_height);

void tako_terminal_session_write(TakoTerminalSession *session,
                                 const uint8_t *data,
                                 uintptr_t len);
uint32_t tako_terminal_session_take_bell_count(TakoTerminalSession *session);
uintptr_t tako_terminal_session_hyperlink_at(TakoTerminalSession *session,
                                             float x_px,
                                             float y_px,
                                             uint8_t *out_buf,
                                             uintptr_t cap);
void tako_terminal_session_paste(TakoTerminalSession *session,
                                 const uint8_t *data,
                                 uintptr_t len);
void tako_terminal_session_scroll(TakoTerminalSession *session, int64_t delta_rows);
void tako_terminal_session_scroll_to_top(TakoTerminalSession *session);
void tako_terminal_session_scroll_to_bottom(TakoTerminalSession *session);
void tako_terminal_session_scroll_to_row(TakoTerminalSession *session, uint64_t row);
bool tako_terminal_session_scrollbar_state(TakoTerminalSession *session,
                                           TakoTerminalScrollbarState *out);

int32_t tako_terminal_session_mouse_tracking(TakoTerminalSession *session);
void tako_terminal_session_mouse_set_any_button(TakoTerminalSession *session, bool pressed);
void tako_terminal_session_key_event(TakoTerminalSession *session,
                                     uint32_t action,
                                     uint32_t key,
                                     uint16_t mods,
                                     uint16_t consumed_mods,
                                     const uint8_t *text,
                                     uintptr_t text_len);
void tako_terminal_session_mouse_event(TakoTerminalSession *session,
                                       uint32_t action,
                                       uint32_t button,
                                       float x_px,
                                       float y_px,
                                       uint16_t mods);

void tako_terminal_session_selection_begin(TakoTerminalSession *session,
                                           float x_px,
                                           float y_px,
                                           uint64_t time_ns,
                                           uint16_t mods,
                                           uint32_t single_click,
                                           uint32_t double_click,
                                           uint32_t triple_click);
int32_t tako_terminal_session_selection_extend(TakoTerminalSession *session,
                                               float x_px,
                                               float y_px,
                                               uint16_t mods);
int32_t tako_terminal_session_selection_autoscroll(TakoTerminalSession *session);
int32_t tako_terminal_session_selection_autoscroll_tick(TakoTerminalSession *session,
                                                        float x_px,
                                                        float y_px,
                                                        uint16_t mods);
uintptr_t tako_terminal_session_selection_end(TakoTerminalSession *session,
                                              float x_px,
                                              float y_px,
                                              uint8_t *out_buf,
                                              uintptr_t cap);
uintptr_t tako_terminal_session_selection_text(TakoTerminalSession *session,
                                               uint8_t *out_buf,
                                               uintptr_t cap);
TakoTerminalBytes tako_terminal_session_selection_end_owned(TakoTerminalSession *session,
                                                            float x_px,
                                                            float y_px);
TakoTerminalBytes tako_terminal_session_selection_text_owned(TakoTerminalSession *session);
void tako_terminal_session_selection_clear(TakoTerminalSession *session);
int32_t tako_terminal_session_selection_all(TakoTerminalSession *session);
int32_t tako_terminal_session_selection_output_at(TakoTerminalSession *session,
                                                  float x_px,
                                                  float y_px);
int32_t tako_terminal_session_selection_input_at(TakoTerminalSession *session,
                                                 float x_px,
                                                 float y_px);
int32_t tako_terminal_session_selection_adjust(TakoTerminalSession *session,
                                               uint32_t adjustment);

void tako_terminal_session_focus_event(TakoTerminalSession *session, bool gained);
uintptr_t tako_terminal_session_take_title(TakoTerminalSession *session,
                                           uint8_t *out_buf,
                                           uintptr_t cap);
uintptr_t tako_terminal_session_take_pwd(TakoTerminalSession *session,
                                         uint8_t *out_buf,
                                         uintptr_t cap);

#ifdef __cplusplus
}  // extern "C"
#endif
