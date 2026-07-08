// TakoTerminalView: a QQuickFramebufferObject hosting a live libghostty-vt
// terminal. Owns the Rust `Surface` on the GUI thread (woken by a
// QSocketNotifier on the readiness pipe, plus a safety timer); the
// render-thread `TakoTerminalRenderer` draws via glow into the Qt-provided FBO.
// See the .cpp for the design and the glow loader bridge.
//
// The FramePlan layout, the Surface/GlRenderer opaque handles, and the C ABI
// the C++ calls all live in the cbindgen-generated `tako_render.h` — there is
// no hand-mirrored struct here.

#pragma once

#include <QtQuick/qquickframebufferobject.h>

#include <cstddef>
#include <cstdint>

#include "tako_render.h"  // FramePlan, Surface, GlRenderer, LoaderFn, C ABI decls

class QKeyEvent;
class QMouseEvent;
class QWheelEvent;
class QFocusEvent;
class QTimer;
class QSocketNotifier;

class TakoTerminalRenderer;

class TakoTerminalView : public QQuickFramebufferObject {
    Q_OBJECT

public:
    explicit TakoTerminalView(QQuickItem *parent = nullptr);
    ~TakoTerminalView() override;

    QQuickFramebufferObject::Renderer *createRenderer() const override;

    // Latest FramePlan (borrowed from the Surface; valid until the next
    // tick). Called by TakoTerminalRenderer::synchronize on the GUI thread.
    const FramePlan &plan() const { return m_plan; }

signals:
    // Emitted once when the hosted PTY session exits. Embedders decide whether
    // to close this view, show restart UI, or quit the application.
    void exited();

protected:
    // Keyboard: translate QKeyEvent to GhosttyKey + mods and forward via the
    // libghostty-vt key encoder (which honors DEC modes / Kitty protocol).
    void keyPressEvent(QKeyEvent *e) override;
    void keyReleaseEvent(QKeyEvent *e) override;

    // Mouse: route to the encoder when mouse tracking is on; otherwise reserve
    // for selection / wheel-scroll (TODO: drag selection).
    void mousePressEvent(QMouseEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;
    void mouseMoveEvent(QMouseEvent *e) override;
    void wheelEvent(QWheelEvent *e) override;
    void focusInEvent(QFocusEvent *e) override;
    void focusOutEvent(QFocusEvent *e) override;

    // Resize: recompute cols/rows from the item size ÷ cell metrics and push
    // to the Surface (which resizes the terminal + PTY).
    void geometryChange(const QRectF &newGeometry, const QRectF &oldGeometry) override;

    // Detect when the item joins a window so we can wire DPR-change signals.
    void itemChange(ItemChange change, const ItemChangeData &value) override;

private:
    void ensureSurface();
    // Read the window's current device-pixel ratio. Falls back to 1.0 when the
    // item isn't in a window yet (e.g. during construction).
    float windowDpr() const;
    // React to a DPR change (window moved between monitors, or the screen's
    // DPR changed): reload the font at the new physical size and reflow.
    void onDprChanged();
    // Pull a fresh title (if any) from the surface and emit windowTitleChanged.
    void flushHostTitle();
    // Drain the readiness pipe + tick the surface; `update()` only if it
    // produced a new frame. Driven by both the wake notifier and the safety
    // timer.
    void pumpAndRender();

    Surface *m_surface = nullptr;  // owned; freed via tako_surface_destroy
    QTimer *m_timer = nullptr;
    QSocketNotifier *m_notifier = nullptr;
    FramePlan m_plan = {};
    bool m_dprSignalConnected = false;
    bool m_exited = false;
    // Tracks whether any mouse button is held, for any-event motion reporting.
    bool m_anyMouseButtonHeld = false;
};

// Render-thread renderer. A thin shell around the Rust GlRenderer: C++ only
// provides the glow GL loader bridge (via QOpenGLContext) and computes the
// physical-pixel viewport from item size × DPR.
class TakoTerminalRenderer : public QQuickFramebufferObject::Renderer {
public:
    TakoTerminalRenderer();
    ~TakoTerminalRenderer() override;

    void synchronize(QQuickFramebufferObject *item) override;
    void render() override;

private:
    GlRenderer *m_gl = nullptr;  // owned; freed via tako_gl_renderer_destroy
    bool m_glInited = false;
};
