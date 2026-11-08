#ifndef ONDA_UI_H
#define ONDA_UI_H
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct OndaUi OndaUi;
typedef struct OndaUiFramePacer OndaUiFramePacer;
enum OndaUiPresentMode {
    ONDA_UI_AUTO_VSYNC = 0, ONDA_UI_AUTO_NO_VSYNC = 1,
    ONDA_UI_FIFO = 2, ONDA_UI_FIFO_RELAXED = 3,
    ONDA_UI_IMMEDIATE = 4, ONDA_UI_MAILBOX = 5,
    ONDA_UI_MAILBOX_OR_FIFO = 6
};
/* Optional host-owned helper. 0 Hz = uncapped; negative/non-finite rates fail.
 * Caller timestamps are finite, monotonic milliseconds. No internal clock,
 * timers, retry policy, or connection to an OndaUi instance. */
OndaUiFramePacer *onda_ui_frame_pacer_create(double rate_hz);
void onda_ui_frame_pacer_destroy(OndaUiFramePacer *pacer);
/* Choose one pair for the pacer's lifetime. Queries return 0 when ready.
 * Timer pair: round positive delays UP to timer resolution. After a successful
 * render, record the host-clock time captured immediately BEFORE rendering,
 * never completion time. Neither pair measures when pixels reach the display. */
double onda_ui_frame_pacer_timer_delay_ms(const OndaUiFramePacer *pacer, double now_ms);
void onda_ui_frame_pacer_timer_submitted(OndaUiFramePacer *pacer, double started_ms);
/* Display pair: arm a display callback before the next slot opens, then check
 * its display timestamp, not callback execution time. After a successful render,
 * record that same display timestamp. Do not mix timer and display pairs. */
double onda_ui_frame_pacer_display_delay_ms(const OndaUiFramePacer *pacer, double now_ms);
void onda_ui_frame_pacer_display_submitted(OndaUiFramePacer *pacer, double display_ms);
/* Complete host protocol and lifecycle reference: docs/ui-api.md in the SDK. */
/* Call all functions on the host UI thread. The native view and display must
 * outlive OndaUi. Coordinates use logical pixels; dimensions use physical pixels.
 * platform: 1 = Xlib (window XID, display Display*), 2 = Win32 HWND,
 * 3 = macOS NSView*. Creation reports failures in the provided UTF-8 buffer.
 * Creation copies options; the struct need not outlive the call. */
typedef struct OndaUiCreateOptions {
    uint32_t platform;
    uintptr_t window;
    uintptr_t display;
    uint32_t width;
    uint32_t height;
    float scale;
    uint32_t present_mode;
    uint32_t maximum_frame_latency;
} OndaUiCreateOptions;
OndaUi *onda_ui_create(const OndaUiCreateOptions *options,
                      char *error, size_t error_capacity);
/* Supported concrete modes: bit (1 << mode). Query returns the resolved mode.
 * Explicit unsupported modes fail. AUTO modes and MAILBOX_OR_FIFO allow fallback;
 * MAILBOX_OR_FIFO chooses tear-free mailbox when supported, otherwise FIFO.
 * Latency is a positive, backend-clamped buffering hint, independent of FPS. */
uint32_t onda_ui_present_modes(const OndaUi *ui);
uint32_t onda_ui_present_mode(const OndaUi *ui);
int onda_ui_set_presentation(OndaUi *ui, uint32_t mode, uint32_t maximum_frame_latency);
void onda_ui_destroy(OndaUi *ui);
void onda_ui_resize(OndaUi *ui, uint32_t width, uint32_t height, float scale);
/* Host-message JSON defined in docs/ui-api.md. Returns 1 on success, 0 on error. */
int onda_ui_message(OndaUi *ui, const char *json);
/* 1 = consumed, 0 = unused (forward keyboard input to the host), -1 = error. */
int onda_ui_input(OndaUi *ui, const char *json);
/* Interleaved bounded visualization snapshot. The UI copies samples before return. */
void onda_ui_scope(OndaUi *ui, uint32_t channels, const float *samples, size_t count);
/* Advance timed UI state/layout without drawing. 1 = redraw needed, 0 = clean,
 * -1 = error. Drain commands afterward, even when clean. */
int onda_ui_update(OndaUi *ui);
/* Delay until the next UI update in milliseconds; -1 = no timed work.
 * Recompute after updates, command draining, or state/input changes. A wakeup
 * need not draw. Use a host timer rounded UP, then update and drain commands.
 * Host data polling and display pacing are independent of this deadline. */
double onda_ui_wake_delay_ms(const OndaUi *ui);
/* Explicit draw, even when clean: 1 = presented, 0 = surface unavailable,
 * -1 = error. No software pacing. Acquisition/presentation may wait for GPU/vsync. */
int onda_ui_render(OndaUi *ui);
/* Whether the clipped scope viewport needs fresh snapshots. Resolves pending layout. */
int onda_ui_scope_visible(OndaUi *ui);
/* Peak maxima: one per channel, up to 64 channels. */
void onda_ui_meter(OndaUi *ui, uint32_t channels, const float *peaks, size_t count);
/* Whether any clipped header meter needs fresh peaks. Resolves pending layout. */
int onda_ui_meter_visible(OndaUi *ui);
/* JSON command array, borrowed until the next call to commands or destroy.
 * Drain after input/state updates, timed updates, and rendering.
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
