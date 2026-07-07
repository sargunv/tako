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
#include <QTimer>
#include <QtQuick/qquickwindow.h>

#include <chrono>
#include <cstdio>

// ---- C ABI into the Rust `tako_render` crate ----

extern "C" {
// surface.rs
void *tako_surface_new(uint16_t cols, uint16_t rows, const char *font_path,
                       uint32_t pixel_height);
void tako_surface_destroy(void *surface);
void tako_surface_tick(void *surface, TakoFramePlan *out);
void tako_surface_write(void *surface, const uint8_t *data, size_t len);

// gl_renderer.rs
void *tako_gl_renderer_new();
void tako_gl_renderer_destroy(void *renderer);
void tako_gl_renderer_ensure_gl(void *renderer,
                                const void *(*loader)(const char *, void *),
                                void *loader_userdata);
void tako_gl_renderer_ingest_plan(void *renderer, const TakoFramePlan *plan,
                                  int32_t viewport_w, int32_t viewport_h);
void tako_gl_renderer_render(void *renderer);

// qml_init.rs
void tako_register_qml_types();
}

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
    // Click-to-focus so subsequent key events arrive here. Middle button is
    // reserved for selection paste (TODO phase-1-§5).
    setAcceptedMouseButtons(Qt::LeftButton | Qt::MiddleButton);
    m_timer = new QTimer(this);
    m_timer->setInterval(16);  // ~60 Hz poll for PTY output
    connect(m_timer, &QTimer::timeout, this, [this] {
        if (m_surface) {
            tako_surface_tick(m_surface, &m_plan);
        }
        update();  // schedule a render (synchronize + render on the SG thread)
    });
    m_timer->start();
}

TakoTerminalView::~TakoTerminalView() {
    if (m_surface) tako_surface_destroy(m_surface);
}

void TakoTerminalView::ensureSurface() {
    if (!m_surface) {
        // 18 px cell height, fixed 80×24 grid for now; resize lands in P3.
        m_surface = tako_surface_new(80, 24, nullptr, 18);
    }
}

QQuickFramebufferObject::Renderer *TakoTerminalView::createRenderer() const {
    const_cast<TakoTerminalView *>(this)->ensureSurface();
    return new TakoTerminalRenderer();
}

void TakoTerminalView::mousePressEvent(QMouseEvent *e) {
    forceActiveFocus();
    e->accept();
    // TODO(phase-1-§5): drag selection (anchor + extent → cell range →
    // PRIMARY selection), middle-click paste, SGR mouse forwarding.
}

// ---- keyboard input ----
//
// Translate QKeyEvent into bytes a terminal expects and feed the PTY. Full
// modifier-aware CSI / Kitty protocol lands in P5 via libghostty-vt's key
// encoder; this hand-rolled table is the spike.

void TakoTerminalView::keyPressEvent(QKeyEvent *e) {
    if (!m_surface) {
        QQuickFramebufferObject::keyPressEvent(e);
        return;
    }

    const int key = e->key();
    const Qt::KeyboardModifiers mods = e->modifiers();
    const QString text = e->text();
    QByteArray bytes;

    const bool ctrl = mods & Qt::ControlModifier;

    if (ctrl && key >= Qt::Key_A && key <= Qt::Key_Z) {
        bytes.append(static_cast<char>(key - Qt::Key_A + 1));
    } else if (ctrl && key == Qt::Key_BracketLeft) {
        bytes.append('\x1b');
    } else if (ctrl && key == Qt::Key_Backslash) {
        bytes.append('\x1c');
    } else if (ctrl && key == Qt::Key_BracketRight) {
        bytes.append('\x1d');
    } else if (ctrl && (key == Qt::Key_At || key == Qt::Key_Space)) {
        bytes.append('\x00');
    } else {
        switch (key) {
            case Qt::Key_Return:
            case Qt::Key_Enter:
                bytes.append('\r');
                break;
            case Qt::Key_Backspace:
                bytes.append('\x7f');
                break;
            case Qt::Key_Tab:
                bytes.append('\t');
                break;
            case Qt::Key_Escape:
                bytes.append('\x1b');
                break;
            case Qt::Key_Up:
                bytes.append("\x1b[A");
                break;
            case Qt::Key_Down:
                bytes.append("\x1b[B");
                break;
            case Qt::Key_Right:
                bytes.append("\x1b[C");
                break;
            case Qt::Key_Left:
                bytes.append("\x1b[D");
                break;
            case Qt::Key_Home:
                bytes.append("\x1b[H");
                break;
            case Qt::Key_End:
                bytes.append("\x1b[F");
                break;
            case Qt::Key_PageUp:
                bytes.append("\x1b[5~");
                break;
            case Qt::Key_PageDown:
                bytes.append("\x1b[6~");
                break;
            case Qt::Key_Insert:
                bytes.append("\x1b[2~");
                break;
            case Qt::Key_Delete:
                bytes.append("\x1b[3~");
                break;
            case Qt::Key_F1:
                bytes.append("\x1bOP");
                break;
            case Qt::Key_F2:
                bytes.append("\x1bOQ");
                break;
            case Qt::Key_F3:
                bytes.append("\x1bOR");
                break;
            case Qt::Key_F4:
                bytes.append("\x1bOS");
                break;
            case Qt::Key_F5:
                bytes.append("\x1b[15~");
                break;
            case Qt::Key_F6:
                bytes.append("\x1b[17~");
                break;
            case Qt::Key_F7:
                bytes.append("\x1b[18~");
                break;
            case Qt::Key_F8:
                bytes.append("\x1b[19~");
                break;
            case Qt::Key_F9:
                bytes.append("\x1b[20~");
                break;
            case Qt::Key_F10:
                bytes.append("\x1b[21~");
                break;
            case Qt::Key_F11:
                bytes.append("\x1b[23~");
                break;
            case Qt::Key_F12:
                bytes.append("\x1b[24~");
                break;
            default:
                if (!text.isEmpty()) {
                    bytes.append(text.toUtf8());
                } else {
                    QQuickFramebufferObject::keyPressEvent(e);
                    return;
                }
        }
    }

    if (!bytes.isEmpty()) {
        tako_surface_write(m_surface,
                           reinterpret_cast<const uint8_t *>(bytes.constData()),
                           static_cast<size_t>(bytes.size()));
    }
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
