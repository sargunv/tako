// TakoTerminalView implementation. See the header for the design.
//
// The C ABI surface (tako_render::surface) is declared here and matched
// field-for-field against the Rust `#[repr(C)]` types.

#include "tako_terminal_view.h"

#include <QColor>
#include <QImage>
#include <QQmlEngine>
#include <QQuickWindow>
#include <QSGFlatColorMaterial>
#include <QSGGeometry>
#include <QSGGeometryNode>
#include <QSGTexture>
#include <QSGTextureMaterial>
#include <QSGTransformNode>
#include <QTimer>
#include <QtQuick/qsgnode.h>

// ---- C ABI into the Rust `tako_render::surface` module ----

extern "C" {
struct TakoCQuad {
    float x, y, w, h;
    float u0, v0, u1, v1;
    uint8_t r, g, b, a;
};
struct TakoCRect {
    float x, y, w, h;
    uint8_t r, g, b, a;
};
struct TakoFramePlan {
    TakoCRect bg;
    TakoCRect cursor;
    uint8_t fg_default[4];
    float cell_w, cell_h;
    uint32_t cols, rows;
    const TakoCQuad *quads;
    size_t quad_count;
    uint32_t atlas_w, atlas_h;
    const uint8_t *atlas_pixels;
};

void *tako_surface_new(uint16_t cols, uint16_t rows, const char *font_path,
                       uint32_t pixel_height);
void tako_surface_destroy(void *surface);
void tako_surface_tick(void *surface, TakoFramePlan *out);
}

// Register the TakoTerminalView QQuickItem as `TerminalView` under a dedicated
// URI. cxx-qt-build's compiled `org.tako` module only registers bridge types,
// so hand-written C++ items are registered imperatively here. Call once before
// loading QML.
extern "C" void tako_register_qml_types() {
    qmlRegisterType<TakoTerminalView>("org.tako.terminal", 1, 0, "TerminalView");
}

namespace {
// Vertex counts are small (≤ a few thousand); UInt16 indices suffice.
constexpr int kCellsMax = 1 << 14;

// Build (or resize) a Point2D geometry for a flat-colored quad.
QSGGeometry *makePointGeometry() {
    auto *g = new QSGGeometry(QSGGeometry::defaultAttributes_Point2D(), 4, 6);
    g->setDrawingMode(QSGGeometry::DrawTriangles);
    auto *ix = g->indexDataAsUShort();
    ix[0] = 0;
    ix[1] = 1;
    ix[2] = 2;
    ix[3] = 0;
    ix[4] = 2;
    ix[5] = 3;
    return g;
}
}  // namespace

TakoTerminalView::TakoTerminalView(QQuickItem *parent) : QQuickItem(parent) {
    setFlag(QQuickItem::ItemHasContents);
    m_timer = new QTimer(this);
    m_timer->setInterval(16);  // ~60 Hz poll for PTY output
    connect(m_timer, &QTimer::timeout, this, [this] { update(); });
    m_timer->start();
}

TakoTerminalView::~TakoTerminalView() {
    delete m_atlasTexture;
    if (m_surface) tako_surface_destroy(m_surface);
}

void TakoTerminalView::itemChange(ItemChange change,
                                  const ItemChangeData &value) {
    QQuickItem::itemChange(change, value);
    if (change == ItemSceneChange && value.window) {
        // Window is now available for texture creation.
        update();
    }
}

void TakoTerminalView::ensureSurface() {
    if (!m_surface) {
        // 18px cell height for the spike; cols/rows fixed at 80x24.
        m_surface = tako_surface_new(80, 24, nullptr, 18);
    }
}

QSGNode *TakoTerminalView::updatePaintNode(QSGNode *oldNode,
                                           UpdatePaintNodeData *) {
    ensureSurface();
    if (!m_surface) return oldNode;

    TakoFramePlan plan;
    tako_surface_tick(m_surface, &plan);

    auto *root = static_cast<QSGTransformNode *>(oldNode);
    if (!root) {
        root = new QSGTransformNode;
        // child 0: background, child 1: cursor, child 2: glyphs.
        auto *bg = new QSGGeometryNode;
        bg->setGeometry(makePointGeometry());
        bg->setFlag(QSGNode::OwnsGeometry);
        bg->setMaterial(new QSGFlatColorMaterial);
        bg->setFlag(QSGNode::OwnsMaterial);
        root->appendChildNode(bg);

        auto *cur = new QSGGeometryNode;
        cur->setGeometry(makePointGeometry());
        cur->setFlag(QSGNode::OwnsGeometry);
        cur->setMaterial(new QSGFlatColorMaterial);
        cur->setFlag(QSGNode::OwnsMaterial);
        root->appendChildNode(cur);

        auto *gly = new QSGGeometryNode;
        gly->setGeometry(new QSGGeometry(
            QSGGeometry::defaultAttributes_TexturedPoint2D(), 0, 0));
        gly->setFlag(QSGNode::OwnsGeometry);
        gly->setMaterial(new QSGTextureMaterial);
        gly->setFlag(QSGNode::OwnsMaterial);
        root->appendChildNode(gly);
    }

    auto *bgNode = static_cast<QSGGeometryNode *>(root->childAtIndex(0));
    auto *cursorNode = static_cast<QSGGeometryNode *>(root->childAtIndex(1));
    auto *glyphNode = static_cast<QSGGeometryNode *>(root->childAtIndex(2));

    // --- background ---
    {
        auto *mat =
            static_cast<QSGFlatColorMaterial *>(bgNode->material());
        mat->setColor(
            QColor(plan.bg.r, plan.bg.g, plan.bg.b, plan.bg.a));
        auto *pts = bgNode->geometry()->vertexDataAsPoint2D();
        const float W = float(plan.cols) * plan.cell_w;
        const float H = float(plan.rows) * plan.cell_h;
        pts[0].set(0, 0);
        pts[1].set(W, 0);
        pts[2].set(W, H);
        pts[3].set(0, H);
        bgNode->markDirty(QSGNode::DirtyGeometry | QSGNode::DirtyMaterial);
    }

    // --- atlas texture (rebuild only when it grows) ---
    if (plan.atlas_w && plan.atlas_h &&
        (int)plan.atlas_w * (int)plan.atlas_h > 0 &&
        (!m_atlasTexture || (int)plan.atlas_w != m_atlasW ||
         (int)plan.atlas_h != m_atlasH)) {
        delete m_atlasTexture;
        m_atlasTexture = nullptr;

        // Bake the default fg color into an RGBA texture using the grayscale
        // atlas as coverage. (Per-cell color needs a custom shader; see header.)
        QImage img(plan.atlas_w, plan.atlas_h, QImage::Format_RGBA8888);
        const uint8_t fr = plan.fg_default[0];
        const uint8_t fg = plan.fg_default[1];
        const uint8_t fb = plan.fg_default[2];
        for (uint32_t y = 0; y < plan.atlas_h; ++y) {
            auto *line = img.scanLine(y);
            const auto *src = plan.atlas_pixels + y * plan.atlas_w;
            for (uint32_t x = 0; x < plan.atlas_w; ++x) {
                const uint8_t cov = src[x];
                line[x * 4 + 0] = fr;
                line[x * 4 + 1] = fg;
                line[x * 4 + 2] = fb;
                line[x * 4 + 3] = cov;
            }
        }
        m_atlasTexture = window()->createTextureFromImage(img);
        m_atlasW = (int)plan.atlas_w;
        m_atlasH = (int)plan.atlas_h;
    }

    // --- glyphs ---
    {
        const int vcount = int(plan.quad_count) * 4;
        const int icount = int(plan.quad_count) * 6;
        QSGGeometry *g = glyphNode->geometry();
        g->allocate(vcount, icount);
        g->setDrawingMode(QSGGeometry::DrawTriangles);
        auto *pts = g->vertexDataAsTexturedPoint2D();
        auto *ix = g->indexDataAsUShort();
        for (size_t i = 0; i < plan.quad_count && i < (size_t)kCellsMax; ++i) {
            const auto &q = plan.quads[i];
            const float x0 = q.x, y0 = q.y;
            const float x1 = q.x + q.w, y1 = q.y + q.h;
            pts[i * 4 + 0].set(x0, y0, q.u0, q.v0);
            pts[i * 4 + 1].set(x1, y0, q.u1, q.v0);
            pts[i * 4 + 2].set(x1, y1, q.u1, q.v1);
            pts[i * 4 + 3].set(x0, y1, q.u0, q.v1);
            const uint16_t b = uint16_t(i * 4);
            ix[i * 6 + 0] = b + 0;
            ix[i * 6 + 1] = b + 1;
            ix[i * 6 + 2] = b + 2;
            ix[i * 6 + 3] = b + 0;
            ix[i * 6 + 4] = b + 2;
            ix[i * 6 + 5] = b + 3;
        }
        auto *mat = static_cast<QSGTextureMaterial *>(glyphNode->material());
        if (m_atlasTexture) mat->setTexture(m_atlasTexture);
        glyphNode->markDirty(QSGNode::DirtyGeometry | QSGNode::DirtyMaterial);
    }

    // --- cursor ---
    {
        auto *mat =
            static_cast<QSGFlatColorMaterial *>(cursorNode->material());
        mat->setColor(
            QColor(plan.cursor.r, plan.cursor.g, plan.cursor.b, plan.cursor.a));
        auto *pts = cursorNode->geometry()->vertexDataAsPoint2D();
        const float cx = plan.cursor.x, cy = plan.cursor.y;
        const float cw = plan.cursor.w, ch = plan.cursor.h;
        pts[0].set(cx, cy);
        pts[1].set(cx + cw, cy);
        pts[2].set(cx + cw, cy + ch);
        pts[3].set(cx, cy + ch);
        cursorNode->markDirty(QSGNode::DirtyGeometry | QSGNode::DirtyMaterial);
    }

    return root;
}
