#ifndef ONDA_UI_H
#define ONDA_UI_H
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct OndaUi OndaUi;
/* Call all functions on the host UI thread. The native view and display must
 * outlive OndaUi. Coordinates use logical pixels; dimensions use physical pixels.
 * platform: 1 = Xlib (window XID, display Display*), 2 = Win32 HWND,
 * 3 = macOS NSView*. Creation reports failures in the provided UTF-8 buffer. */
OndaUi *onda_ui_create(uint32_t platform, uintptr_t window, uintptr_t display,
                      uint32_t width, uint32_t height, float scale,
                      char *error, size_t error_capacity);
void onda_ui_destroy(OndaUi *ui);
void onda_ui_resize(OndaUi *ui, uint32_t width, uint32_t height, float scale);
/* Existing run-view JSON messages, or input events as documented in ui/gpu. */
int onda_ui_message(OndaUi *ui, const char *json);
/* 1 = consumed, 0 = unused (forward keyboard input to the host), -1 = error. */
int onda_ui_input(OndaUi *ui, const char *json);
/* Interleaved bounded visualization snapshot. The UI copies samples before return. */
void onda_ui_scope(OndaUi *ui, uint32_t channels, const float *samples, size_t count);
/* 1 = presented/clean, 0 = retry later, -1 = error. */
int onda_ui_render(OndaUi *ui);
/* Whether the visible scope panel needs fresh snapshots. */
int onda_ui_scope_visible(const OndaUi *ui);
/* JSON command array, borrowed until the next call to commands or destroy.
 * Drain after input/state updates and on UI timer ticks for deferred scroll saves.
 * Send focusLost and drain commands before hiding or destroying the view. */
const char *onda_ui_commands(OndaUi *ui);
const char *onda_ui_error(const OndaUi *ui);
/* Native text/IME state: UTF-8 text, UTF-16 selection offsets, logical rect.
 * Each snapshot is borrowed until the next call to that function or destroy. */
const char *onda_ui_text_edit(OndaUi *ui);
/* Shaped text geometry in logical pixels. Offsets are UTF-16 code units.
 * bounds writes four floats (x,y,width,height); hit returns SIZE_MAX without an editor. */
int onda_ui_text_bounds(OndaUi *ui, size_t start, size_t end, float rect[4]);
size_t onda_ui_text_hit(OndaUi *ui, float x, float y);
/* CSS cursor identifier (default, pointer, text, ew-resize, ns-resize), static lifetime. */
const char *onda_ui_cursor(OndaUi *ui);
#ifdef __cplusplus
}
#endif
#endif
