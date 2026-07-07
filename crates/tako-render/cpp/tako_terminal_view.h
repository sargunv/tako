// TakoTerminalView: a QQuickFramebufferObject hosting a live libghostty-vt
// terminal. Owns the Rust `Surface` on the GUI thread (driven by a 16 ms
// QTimer); the render-thread `TakoTerminalRenderer` draws via glow into the
// Qt-provided FBO. See the .cpp for the design and the glow loader bridge.
//
// C ABI surface (tako_render::{surface, gl_renderer}): the item ticks the
// Surface and stores the latest FramePlan; the Renderer::synchronize (GUI
// thread) deep-copies it into the GlRenderer's staging; Renderer::render
// (render thread) issues the GL draws.

#pragma once

#include <QtQuick/qquickframebufferobject.h>

#include <cstddef>
#include <cstdint>

class QKeyEvent;
class QMouseEvent;
class QWheelEvent;
class QFocusEvent;
class QTimer;

// C-layout mirror of `tako_render::surface::FramePlan`. Field-for-field
// identical; bump in lockstep with the Rust definition.
struct TakoFramePlan {
    uint8_t clear_color[4];
    float cell_w, cell_h;
    uint32_t cols, rows;
    const void *vertices;  // *const Vertex (Rust); opaque to C++
    size_t vertex_count;
    uint32_t atlas_w, atlas_h;
    const uint8_t *atlas_pixels;
    uint64_t atlas_generation;
};

class TakoTerminalRenderer;

class TakoTerminalView : public QQuickFramebufferObject {
    Q_OBJECT

public:
    explicit TakoTerminalView(QQuickItem *parent = nullptr);
    ~TakoTerminalView() override;

    QQuickFramebufferObject::Renderer *createRenderer() const override;

    // Latest FramePlan (borrowed from the Surface; valid until the next
    // timer tick). Called by TakoTerminalRenderer::synchronize on the GUI
    // thread.
    const TakoFramePlan &plan() const { return m_plan; }

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

    void *m_surface = nullptr;  // Surface* from tako_surface_new (opaque)
    QTimer *m_timer = nullptr;
    TakoFramePlan m_plan = {};
    bool m_dprSignalConnected = false;
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
    void *m_gl = nullptr;  // GlRenderer* from tako_gl_renderer_new (opaque)
    bool m_glInited = false;
};
