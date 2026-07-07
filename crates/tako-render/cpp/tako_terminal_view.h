// TakoTerminalView: a QQuickItem that renders a live libghostty-vt terminal.
//
// Owns a Rust `Surface` (Terminal + RenderState + PTY + glyph atlas) via the
// C ABI declared in the .cpp. Each frame (driven by a 16ms poll timer) it
// acquires a FramePlan and builds three QSG nodes: a flat background rect,
// a textured node sampling the grayscale glyph atlas (monochrome, tinted with
// the terminal default fg), and a flat cursor rect.
//
// Phase 0 §3 spike: per-cell color requires a custom QSG material + shader
// (deferred); this monochrome path proves the QSGGeometry+QSGTexture pipeline.

#pragma once

#include <QtQuick/qquickitem.h>

class QTimer;

class TakoTerminalView : public QQuickItem {
    Q_OBJECT

public:
    explicit TakoTerminalView(QQuickItem *parent = nullptr);
    ~TakoTerminalView() override;

protected:
    QSGNode *updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *) override;
    void itemChange(ItemChange change, const ItemChangeData &value) override;

private:
    void ensureSurface();

    void *m_surface = nullptr;   // Surface* from tako_surface_new
    QTimer *m_timer = nullptr;
    class QSGTexture *m_atlasTexture = nullptr;
    int m_atlasW = 0;
    int m_atlasH = 0;
};
