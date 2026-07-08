// TakoTerminalView + TakoTerminalRenderer implementation. See the header.
//
// Two C ABI surfaces are consumed here:
//   - tako_render::surface — Surface lifecycle + tick (produces a FramePlan).
//   - tako_render::gl_renderer — GlRenderer lifecycle + GL draws.
//
// Threading model:
//   - TakoTerminalView lives on the GUI thread. Its QTimer calls
//     tako_surface_tick to refresh m_plan, then QQuickItem::update() to
//     schedule a render.
//   - TakoTerminalRenderer::synchronize (GUI thread) copies the latest plan
//     into the GlRenderer's staging. The framework serializes synchronize
//     with render().
//   - TakoTerminalRenderer::render (render thread) lazily attaches the
//     GlRenderer to Qt's current GL context (via a getProcAddress loader
//     bridge) and issues the draw.

#include "tako_terminal_view.h"

#include <QGuiApplication>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QOpenGLContext>
#include <QQuickWindow>
#include <QScreen>
#include <QSocketNotifier>
#include <QTimer>
#include <QWheelEvent>
#include <QtQuick/qquickwindow.h>

#include <chrono>
#include <cstdio>

// libghostty-vt enum values used by the input C ABI. The headers are
// header-only (typedefs + enum constants), so we can include them here
// without linking against the library.
#include <ghostty/vt/key/event.h>
#include <ghostty/vt/mouse/event.h>

// The C ABI (FramePlan, Surface, GlRenderer, LoaderFn, and the
// tako_surface_* / tako_gl_renderer_* declarations) comes from the
// cbindgen-generated `tako_render.h`, pulled in via the header.

// Register TakoTerminalView as `TerminalView` under a dedicated URI. Call once
// before loading QML (see tako_render::qml_init).
extern "C" void tako_register_qml_types() {
    qmlRegisterType<TakoTerminalView>("org.tako.terminal", 1, 0, "TerminalView");
}

// ---- glow loader bridge: resolve GL entry points via Qt ----

namespace {
const void *qt_gl_loader(const char *name, void *userdata) {
    auto *ctx = static_cast<QOpenGLContext *>(userdata);
    if (!ctx) return nullptr;
    return reinterpret_cast<const void *>(ctx->getProcAddress(name));
}
}  // namespace

// ---- TakoTerminalView (GUI thread) ----

TakoTerminalView::TakoTerminalView(QQuickItem *parent)
    : QQuickFramebufferObject(parent) {
    // Click-to-focus; all buttons claimed so we can do middle-click paste,
    // right-click selection extend, etc.
    setAcceptedMouseButtons(Qt::AllButtons);
    setAcceptHoverEvents(true);
    setFlag(ItemAcceptsInputMethod, true);  // IME preedit (TODO: render)
    // Create the surface + readiness notifier HERE, on the GUI thread.
    // QQuickFramebufferObject::createRenderer() runs on the QSG render thread;
    // doing this there would (a) parent a QObject (the notifier) cross-thread
    // and (b) put the !Send Rust Surface on the wrong thread while the
    // GUI-thread timer/notifier tick it. The Surface is GUI-only state.
    ensureSurface();
    // Safety + autorun timer. Output latency is handled by m_notifier (wired
    // in ensureSurface once the surface — and its readiness fd — exists); this
    // timer just keeps the autorun harness ticking and catches any wake the
    // notifier might miss at teardown edges.
    m_timer = new QTimer(this);
    m_timer->setInterval(100);
    connect(m_timer, &QTimer::timeout, this, [this] { pumpAndRender(); });
    m_timer->start();
}

TakoTerminalView::~TakoTerminalView() {
    // Stop watching the readiness fd before the surface (which owns it) is
    // freed — otherwise the notifier would dangle.
    if (m_notifier) m_notifier->setEnabled(false);
    if (m_surface) tako_surface_destroy(m_surface);
}

void TakoTerminalView::pumpAndRender() {
    if (!m_surface) return;
    // Clear any pending wake bytes so the level-triggered notifier settles,
    // then advance the terminal. tick() reports whether a new frame was
    // actually produced; if not, skip update() and the GPU stays idle.
    // (Sizing is event-driven via geometryChange + onDprChanged — no polling
    // here. A DPR change forces a replan inside tick even though cols/rows are
    // unchanged, so the GL viewport refreshes.)
    tako_surface_drain_notify(m_surface);
    if (tako_surface_tick(m_surface, &m_plan)) {
        flushHostTitle();
        update();
    }
    if (!m_exited && tako_surface_exited(m_surface)) {
        m_exited = true;
        emit exited();
    }
}

float TakoTerminalView::windowDpr() const {
    if (auto *w = window()) {
        return static_cast<float>(w->devicePixelRatio());
    }
    return 1.0f;
}

void TakoTerminalView::onDprChanged() {
    if (!m_surface) return;
    const float dpr = windowDpr();
    tako_surface_set_dpr(m_surface, dpr);
    // Reflow to the new cell metrics. Cell metrics are now physical, so pass
    // the item's physical size (DIP × DPR).
    if (width() >= 1.0 && height() >= 1.0) {
        tako_surface_resize_pixels(m_surface,
                                   static_cast<uint32_t>(width() * dpr),
                                   static_cast<uint32_t>(height() * dpr));
    }
    // Re-render immediately: set_dpr reloaded the font (new glyph metrics) and
    // set a forced-replan flag inside the surface, so pump now to rebuild the
    // plan and refresh the GL viewport without waiting for the safety timer.
    pumpAndRender();
}

void TakoTerminalView::itemChange(ItemChange change, const ItemChangeData &value) {
    QQuickFramebufferObject::itemChange(change, value);
    // The authoritative "DPR changed" hook. On Wayland with fractional scaling,
    // the window is created with the integer DPR (e.g. 2) and the compositor's
    // preferred fractional scale (e.g. 1.7) arrives later as a
    // wp_fractional_scale preferred_scale event — Qt surfaces that here, as
    // ItemDevicePixelRatioHasChanged carrying value.realValue. Re-checking on
    // activeFocusItemChanged (the old hack) only caught it incidentally and
    // raced differently per monitor.
    if (change == ItemDevicePixelRatioHasChanged) {
        onDprChanged();
        return;
    }
    if (change == ItemSceneChange && value.window && !m_dprSignalConnected) {
        m_dprSignalConnected = true;
        // Monitor switches (window dragged to a different screen) come through
        // screenChanged; ItemDevicePixelRatioChanged above covers fractional
        // arrivals on the same screen.
        connect(value.window, &QQuickWindow::screenChanged, this,
                [this](QScreen *) { onDprChanged(); });
        onDprChanged();
    }
}

void TakoTerminalView::ensureSurface() {
    if (!m_surface) {
        // Placeholder grid; resized to the item's actual geometry immediately
        // below (or on the first geometryChange if the item isn't laid out
        // yet). 18 px is the LOGICAL cell height; the surface multiplies it by
        // the window's devicePixelRatio to rasterize at physical resolution.
        const float dpr = windowDpr();
        m_surface = tako_surface_new(80, 24, nullptr, 18, dpr);
        // Resize to the item's actual physical size on creation so the grid
        // matches the window from the first frame (cell metrics are physical
        // post-P4, so pass width × dpr).
        if (m_surface && width() >= 1.0 && height() >= 1.0) {
            const uint32_t phys_w = static_cast<uint32_t>(width() * dpr);
            const uint32_t phys_h = static_cast<uint32_t>(height() * dpr);
            tako_surface_resize_pixels(m_surface, phys_w, phys_h);
        }
        // Wire the readiness pipe: wake immediately on PTY output instead of
        // waiting for the safety timer. fd == -1 means the surface couldn't
        // create the pipe (timer-only fallback).
        if (m_surface) {
            const int fd = tako_surface_notify_fd(m_surface);
            if (fd >= 0) {
                m_notifier = new QSocketNotifier(fd, QSocketNotifier::Read, this);
                connect(m_notifier, &QSocketNotifier::activated, this,
                        [this] { pumpAndRender(); });
            }
        }
    }
}

void TakoTerminalView::geometryChange(const QRectF &newGeometry,
                                      const QRectF &oldGeometry) {
    QQuickFramebufferObject::geometryChange(newGeometry, oldGeometry);
    if (!m_surface) {
        return;
    }
    // Skip degenerate sizes (0×0 at startup, or during teardown).
    if (newGeometry.width() < 1.0 || newGeometry.height() < 1.0) {
        return;
    }
    // `newGeometry` is in DIPs; the surface's cell metrics are physical, so
    // pass physical px (DIP × DPR). The surface no-ops if cols/rows are
    // unchanged, so sub-cell motion during drag doesn't trigger a resize.
    const float dpr = windowDpr();
    tako_surface_resize_pixels(
        m_surface, static_cast<uint32_t>(newGeometry.width() * dpr),
        static_cast<uint32_t>(newGeometry.height() * dpr));
}

QQuickFramebufferObject::Renderer *TakoTerminalView::createRenderer() const {
    // The Surface was created in the constructor (GUI thread); this runs on the
    // QSG render thread and only needs to spawn the render-thread Renderer.
    return new TakoTerminalRenderer();
}

// ---- helpers ----

namespace {

// Translate Qt modifier flags to GhosttyMods bitmask (key/event.h).
uint16_t qt_mods_to_ghostty(Qt::KeyboardModifiers q) {
    uint16_t m = 0;
    if (q & Qt::ShiftModifier)    m |= GHOSTTY_MODS_SHIFT;
    if (q & Qt::ControlModifier)  m |= GHOSTTY_MODS_CTRL;
    if (q & Qt::AltModifier)      m |= GHOSTTY_MODS_ALT;
    if (q & Qt::MetaModifier)     m |= GHOSTTY_MODS_SUPER;
    if (q & Qt::KeypadModifier)   m |= GHOSTTY_MODS_NUM_LOCK;  // approx
    if (q & Qt::GroupSwitchModifier) m |= GHOSTTY_MODS_ALT;    // approx (X11)
    return m;
}

uint16_t qt_consumed_mods_to_ghostty(const QKeyEvent *e) {
    // consumed_mods are modifiers Qt used to produce text, not modifiers the
    // terminal program should see. Ctrl/Alt/Super must remain visible so
    // navigation keys encode as CSI 1;5D, CSI 1;3D, etc.
    if (e->text().isEmpty()) return 0;
    return (e->modifiers() & Qt::ShiftModifier) ? GHOSTTY_MODS_SHIFT : 0;
}

// Translate Qt::Key to GhosttyKey (W3C UI Events physical codes). Returns
// GHOSTTY_KEY_UNIDENTIFIED for keys we don't map yet.
GhosttyKey qt_key_to_ghostty(int key) {
    switch (key) {
        // Letters
        case Qt::Key_A: return GHOSTTY_KEY_A;
        case Qt::Key_B: return GHOSTTY_KEY_B;
        case Qt::Key_C: return GHOSTTY_KEY_C;
        case Qt::Key_D: return GHOSTTY_KEY_D;
        case Qt::Key_E: return GHOSTTY_KEY_E;
        case Qt::Key_F: return GHOSTTY_KEY_F;
        case Qt::Key_G: return GHOSTTY_KEY_G;
        case Qt::Key_H: return GHOSTTY_KEY_H;
        case Qt::Key_I: return GHOSTTY_KEY_I;
        case Qt::Key_J: return GHOSTTY_KEY_J;
        case Qt::Key_K: return GHOSTTY_KEY_K;
        case Qt::Key_L: return GHOSTTY_KEY_L;
        case Qt::Key_M: return GHOSTTY_KEY_M;
        case Qt::Key_N: return GHOSTTY_KEY_N;
        case Qt::Key_O: return GHOSTTY_KEY_O;
        case Qt::Key_P: return GHOSTTY_KEY_P;
        case Qt::Key_Q: return GHOSTTY_KEY_Q;
        case Qt::Key_R: return GHOSTTY_KEY_R;
        case Qt::Key_S: return GHOSTTY_KEY_S;
        case Qt::Key_T: return GHOSTTY_KEY_T;
        case Qt::Key_U: return GHOSTTY_KEY_U;
        case Qt::Key_V: return GHOSTTY_KEY_V;
        case Qt::Key_W: return GHOSTTY_KEY_W;
        case Qt::Key_X: return GHOSTTY_KEY_X;
        case Qt::Key_Y: return GHOSTTY_KEY_Y;
        case Qt::Key_Z: return GHOSTTY_KEY_Z;
        // Digits
        case Qt::Key_0: return GHOSTTY_KEY_DIGIT_0;
        case Qt::Key_1: return GHOSTTY_KEY_DIGIT_1;
        case Qt::Key_2: return GHOSTTY_KEY_DIGIT_2;
        case Qt::Key_3: return GHOSTTY_KEY_DIGIT_3;
        case Qt::Key_4: return GHOSTTY_KEY_DIGIT_4;
        case Qt::Key_5: return GHOSTTY_KEY_DIGIT_5;
        case Qt::Key_6: return GHOSTTY_KEY_DIGIT_6;
        case Qt::Key_7: return GHOSTTY_KEY_DIGIT_7;
        case Qt::Key_8: return GHOSTTY_KEY_DIGIT_8;
        case Qt::Key_9: return GHOSTTY_KEY_DIGIT_9;
        // Punctuation
        case Qt::Key_Minus:        return GHOSTTY_KEY_MINUS;
        case Qt::Key_Equal:        return GHOSTTY_KEY_EQUAL;
        case Qt::Key_BracketLeft:  return GHOSTTY_KEY_BRACKET_LEFT;
        case Qt::Key_BracketRight: return GHOSTTY_KEY_BRACKET_RIGHT;
        case Qt::Key_Backslash:    return GHOSTTY_KEY_BACKSLASH;
        case Qt::Key_Semicolon:    return GHOSTTY_KEY_SEMICOLON;
        case Qt::Key_Apostrophe:   return GHOSTTY_KEY_QUOTE;
        case Qt::Key_Comma:        return GHOSTTY_KEY_COMMA;
        case Qt::Key_Period:       return GHOSTTY_KEY_PERIOD;
        case Qt::Key_Slash:        return GHOSTTY_KEY_SLASH;
        case Qt::Key_QuoteLeft:    return GHOSTTY_KEY_BACKQUOTE;
        // Functional / control
        case Qt::Key_Return:   return GHOSTTY_KEY_ENTER;
        case Qt::Key_Enter:    return GHOSTTY_KEY_NUMPAD_ENTER;
        case Qt::Key_Backspace:return GHOSTTY_KEY_BACKSPACE;
        case Qt::Key_Tab:      return GHOSTTY_KEY_TAB;
        case Qt::Key_Space:    return GHOSTTY_KEY_SPACE;
        case Qt::Key_Escape:   return GHOSTTY_KEY_ESCAPE;
        case Qt::Key_CapsLock: return GHOSTTY_KEY_CAPS_LOCK;
        case Qt::Key_Shift:    return GHOSTTY_KEY_SHIFT_LEFT;
        case Qt::Key_Control:  return GHOSTTY_KEY_CONTROL_LEFT;
        case Qt::Key_Alt:      return GHOSTTY_KEY_ALT_LEFT;
        case Qt::Key_Meta:     return GHOSTTY_KEY_META_LEFT;
        // Control pad
        case Qt::Key_Delete:     return GHOSTTY_KEY_DELETE;
        case Qt::Key_Insert:     return GHOSTTY_KEY_INSERT;
        case Qt::Key_Home:       return GHOSTTY_KEY_HOME;
        case Qt::Key_End:        return GHOSTTY_KEY_END;
        case Qt::Key_PageUp:     return GHOSTTY_KEY_PAGE_UP;
        case Qt::Key_PageDown:   return GHOSTTY_KEY_PAGE_DOWN;
        case Qt::Key_Help:       return GHOSTTY_KEY_HELP;
        // Arrow pad
        case Qt::Key_Up:    return GHOSTTY_KEY_ARROW_UP;
        case Qt::Key_Down:  return GHOSTTY_KEY_ARROW_DOWN;
        case Qt::Key_Left:  return GHOSTTY_KEY_ARROW_LEFT;
        case Qt::Key_Right: return GHOSTTY_KEY_ARROW_RIGHT;
        // Numpad (with KeypadModifier)
        case Qt::Key_NumLock:      return GHOSTTY_KEY_NUM_LOCK;
        // Function keys
        case Qt::Key_F1:  return GHOSTTY_KEY_F1;
        case Qt::Key_F2:  return GHOSTTY_KEY_F2;
        case Qt::Key_F3:  return GHOSTTY_KEY_F3;
        case Qt::Key_F4:  return GHOSTTY_KEY_F4;
        case Qt::Key_F5:  return GHOSTTY_KEY_F5;
        case Qt::Key_F6:  return GHOSTTY_KEY_F6;
        case Qt::Key_F7:  return GHOSTTY_KEY_F7;
        case Qt::Key_F8:  return GHOSTTY_KEY_F8;
        case Qt::Key_F9:  return GHOSTTY_KEY_F9;
        case Qt::Key_F10: return GHOSTTY_KEY_F10;
        case Qt::Key_F11: return GHOSTTY_KEY_F11;
        case Qt::Key_F12: return GHOSTTY_KEY_F12;
        case Qt::Key_F13: return GHOSTTY_KEY_F13;
        case Qt::Key_F14: return GHOSTTY_KEY_F14;
        case Qt::Key_F15: return GHOSTTY_KEY_F15;
        case Qt::Key_F16: return GHOSTTY_KEY_F16;
        case Qt::Key_F17: return GHOSTTY_KEY_F17;
        case Qt::Key_F18: return GHOSTTY_KEY_F18;
        case Qt::Key_F19: return GHOSTTY_KEY_F19;
        case Qt::Key_F20: return GHOSTTY_KEY_F20;
        case Qt::Key_F21: return GHOSTTY_KEY_F21;
        case Qt::Key_F22: return GHOSTTY_KEY_F22;
        case Qt::Key_F23: return GHOSTTY_KEY_F23;
        case Qt::Key_F24: return GHOSTTY_KEY_F24;
        case Qt::Key_F25: return GHOSTTY_KEY_F25;
        // Lock / misc
        case Qt::Key_ScrollLock: return GHOSTTY_KEY_SCROLL_LOCK;
        case Qt::Key_Pause:      return GHOSTTY_KEY_PAUSE;
        case Qt::Key_Print:      return GHOSTTY_KEY_PRINT_SCREEN;
        case Qt::Key_Menu:       return GHOSTTY_KEY_CONTEXT_MENU;
        default: return GHOSTTY_KEY_UNIDENTIFIED;
    }
}

// Pick a GhosttyMouseAction for a press/release/move.
uint32_t mouse_action_press()   { return GHOSTTY_MOUSE_ACTION_PRESS; }
uint32_t mouse_action_release() { return GHOSTTY_MOUSE_ACTION_RELEASE; }
uint32_t mouse_action_motion()  { return GHOSTTY_MOUSE_ACTION_MOTION; }

// Translate Qt::MouseButton to GhosttyMouseButton. Returns 0 (UNKNOWN) for
// "no button" (used for motion events).
uint32_t qt_button_to_ghostty(Qt::MouseButtons b) {
    if (b & Qt::LeftButton)   return GHOSTTY_MOUSE_BUTTON_LEFT;
    if (b & Qt::RightButton)  return GHOSTTY_MOUSE_BUTTON_RIGHT;
    if (b & Qt::MiddleButton) return GHOSTTY_MOUSE_BUTTON_MIDDLE;
    if (b & Qt::BackButton)   return GHOSTTY_MOUSE_BUTTON_EIGHT;
    if (b & Qt::ForwardButton)return GHOSTTY_MOUSE_BUTTON_NINE;
    return GHOSTTY_MOUSE_BUTTON_UNKNOWN;
}

}  // namespace

void TakoTerminalView::flushHostTitle() {
    if (!m_surface) return;
    char buf[512];
    size_t n = tako_surface_take_title(
        m_surface, reinterpret_cast<uint8_t *>(buf), sizeof(buf) - 1);
    if (n > 0) {
        buf[n] = '\0';
        if (auto *w = window()) w->setTitle(QString::fromUtf8(buf));
    }
}

void TakoTerminalView::mousePressEvent(QMouseEvent *e) {
    forceActiveFocus();
    if (!m_surface) { e->accept(); return; }

    const bool mouse_tracking =
        tako_surface_mouse_tracking(m_surface) != 0;

    if (mouse_tracking) {
        // Program wants raw events: forward to the encoder.
        const float dpr = windowDpr();
        const QPointF p = e->position();
        tako_surface_mouse_event(
            m_surface, mouse_action_press(), qt_button_to_ghostty(e->button()),
            static_cast<float>(p.x()) * dpr,
            static_cast<float>(p.y()) * dpr,
            qt_mods_to_ghostty(e->modifiers()));
        m_anyMouseButtonHeld = true;
        tako_surface_mouse_set_any_button(m_surface, true);
    } else {
        // Local selection: anchor start point here.
        // TODO: selection engine.
    }
    e->accept();
}

void TakoTerminalView::mouseReleaseEvent(QMouseEvent *e) {
    if (!m_surface) { e->accept(); return; }

    const bool mouse_tracking =
        tako_surface_mouse_tracking(m_surface) != 0;
    if (mouse_tracking) {
        const float dpr = windowDpr();
        const QPointF p = e->position();
        tako_surface_mouse_event(
            m_surface, mouse_action_release(),
            qt_button_to_ghostty(e->button()),
            static_cast<float>(p.x()) * dpr,
            static_cast<float>(p.y()) * dpr,
            qt_mods_to_ghostty(e->modifiers()));
    }
    // Any-button tracking state: recompute from current app mouse buttons.
    const bool any_held =
        (QGuiApplication::mouseButtons() != Qt::NoButton);
    if (any_held != m_anyMouseButtonHeld) {
        m_anyMouseButtonHeld = any_held;
        tako_surface_mouse_set_any_button(m_surface, any_held);
    }
    e->accept();
}

void TakoTerminalView::mouseMoveEvent(QMouseEvent *e) {
    if (!m_surface) { e->accept(); return; }
    const bool mouse_tracking =
        tako_surface_mouse_tracking(m_surface) != 0;
    if (mouse_tracking) {
        const float dpr = windowDpr();
        const QPointF p = e->position();
        tako_surface_mouse_event(
            m_surface, mouse_action_motion(), /*button=*/0,
            static_cast<float>(p.x()) * dpr,
            static_cast<float>(p.y()) * dpr,
            qt_mods_to_ghostty(e->modifiers()));
    }
    // TODO: drag selection when not tracking.
    e->accept();
}

void TakoTerminalView::wheelEvent(QWheelEvent *e) {
    if (!m_surface) { e->accept(); return; }

    // Mouse-wheel scrolling: when mouse tracking is on, encode as button-4/5
    // events. Otherwise, scroll the local viewport directly (alternate-screen
    // applications like less/vim enable mode 1007 / mouse tracking, so this
    // branch is reached only when the program is *not* capturing the wheel).
    const bool mouse_tracking =
        tako_surface_mouse_tracking(m_surface) != 0;
    const QPointF deg = e->angleDelta() / 8.0;
    if (mouse_tracking) {
        // Vertical wheel → button 4 (up) / 5 (down); horizontal → 6 / 7.
        const int vsteps = deg.y() > 0
                               ? (deg.y() / 15.0) + 0.5
                               : -((-deg.y() / 15.0) + 0.5);
        for (int i = 0; i < std::abs(vsteps); ++i) {
            uint32_t btn = (vsteps > 0) ? 4 : 5;  // GHOSTTY_MOUSE_BUTTON_FOUR/FIVE
            const float dpr = windowDpr();
            const QPointF p = e->position();
            tako_surface_mouse_event(
                m_surface, mouse_action_press(), btn,
                static_cast<float>(p.x()) * dpr,
                static_cast<float>(p.y()) * dpr,
                qt_mods_to_ghostty(e->modifiers()));
            tako_surface_mouse_event(
                m_surface, mouse_action_release(), btn,
                static_cast<float>(p.x()) * dpr,
                static_cast<float>(p.y()) * dpr,
                qt_mods_to_ghostty(e->modifiers()));
        }
    } else {
        // Scroll the viewport by lines. ±15 degrees ≈ one notch = 3 lines
        // (xterm default).
        const int lines = static_cast<int>(std::round(deg.y() / 5.0));
        if (lines != 0) {
            tako_surface_scroll(m_surface, /*delta_rows=*/-lines);
        }
    }
    e->accept();
}

void TakoTerminalView::focusInEvent(QFocusEvent *e) {
    if (m_surface) tako_surface_focus_event(m_surface, /*gained=*/true);
    e->accept();
}

void TakoTerminalView::focusOutEvent(QFocusEvent *e) {
    if (m_surface) tako_surface_focus_event(m_surface, /*gained=*/false);
    e->accept();
}

// ---- keyboard input ----
//
// Translate QKeyEvent to a GhosttyKey + mods and hand off to libghostty-vt's
// encoder (which honors DEC modes 1, 66, 1036, modifyOtherKeys, Kitty
// keyboard, etc. — no hand-rolled tables on our side).

void TakoTerminalView::keyPressEvent(QKeyEvent *e) {
    if (!m_surface) {
        QQuickFramebufferObject::keyPressEvent(e);
        return;
    }

    const GhosttyKey key = qt_key_to_ghostty(e->key());
    if (key == GHOSTTY_KEY_UNIDENTIFIED) {
        // Fallback: if the event carries printable text, send it raw.
        const QString text = e->text();
        if (!text.isEmpty()) {
            const QByteArray bytes = text.toUtf8();
            tako_surface_write(
                m_surface,
                reinterpret_cast<const uint8_t *>(bytes.constData()),
                static_cast<size_t>(bytes.size()));
            e->accept();
            return;
        }
        QQuickFramebufferObject::keyPressEvent(e);
        return;
    }

    const uint16_t mods = qt_mods_to_ghostty(e->modifiers());
    const uint16_t consumed_mods = qt_consumed_mods_to_ghostty(e);

    // UTF-8 text the key produced. The encoder strips C0 controls for us.
    const QString text = e->text();
    const QByteArray text_bytes = text.toUtf8();
    const uint8_t *text_ptr =
        text_bytes.isEmpty() ? nullptr
                              : reinterpret_cast<const uint8_t *>(text_bytes.constData());
    const size_t text_len = static_cast<size_t>(text_bytes.size());

    // Action: PRESS / REPEAT / RELEASE.
    uint32_t action = GHOSTTY_KEY_ACTION_PRESS;
    if (e->isAutoRepeat()) action = GHOSTTY_KEY_ACTION_REPEAT;

    tako_surface_key_event(m_surface, action, static_cast<uint32_t>(key),
                           mods, consumed_mods, text_ptr, text_len);
    e->accept();
}

void TakoTerminalView::keyReleaseEvent(QKeyEvent *e) {
    if (!m_surface) {
        QQuickFramebufferObject::keyReleaseEvent(e);
        return;
    }
    // Qt sometimes emits autorepeat on release; libghostty-vt wants a true
    // release only on the final key-up.
    if (e->isAutoRepeat()) {
        e->accept();
        return;
    }
    const GhosttyKey key = qt_key_to_ghostty(e->key());
    if (key == GHOSTTY_KEY_UNIDENTIFIED) {
        e->accept();
        return;
    }
    const uint16_t mods = qt_mods_to_ghostty(e->modifiers());
    tako_surface_key_event(m_surface, GHOSTTY_KEY_ACTION_RELEASE,
                           static_cast<uint32_t>(key), mods, 0,
                           nullptr, 0);
    e->accept();
}

// ---- TakoTerminalRenderer (render thread) ----
//
// A thin shell around the Rust GlRenderer. C++ only does two things the Rust
// side can't: (1) provide the glow GL loader bridge via QOpenGLContext, and
// (2) compute the physical-pixel viewport from the item's logical size × DPR.

TakoTerminalRenderer::TakoTerminalRenderer() : m_gl(tako_gl_renderer_new()) {}

TakoTerminalRenderer::~TakoTerminalRenderer() {
    if (m_gl) tako_gl_renderer_destroy(m_gl);
}

void TakoTerminalRenderer::synchronize(QQuickFramebufferObject *item) {
    auto *view = static_cast<TakoTerminalView *>(item);
    // Physical-pixel viewport: item logical size × window DPR. The FBO Qt
    // creates for us is sized in physical pixels (textureFollowsItemSize).
    const QSizeF logical = view->size();
    const qreal dpr = view->window() ? view->window()->devicePixelRatio() : 1.0;
    const int vw = static_cast<int>(logical.width() * dpr);
    const int vh = static_cast<int>(logical.height() * dpr);
    tako_gl_renderer_ingest_plan(m_gl, &view->plan(), vw, vh);
}

void TakoTerminalRenderer::render() {
    // Lazy GL init: glow needs QOpenGLContext::currentContext() to resolve
    // function pointers, which is only valid here on the render thread.
    if (!m_glInited) {
        tako_gl_renderer_ensure_gl(m_gl, qt_gl_loader, QOpenGLContext::currentContext());
        m_glInited = true;
    }
    tako_gl_renderer_render(m_gl);
}
