// TakoTerminalView implementation. See the header for the design.
//
// The C ABI surface (tako_render::surface) is declared here and matched
// field-for-field against the Rust `#[repr(C)]` types.

#include "tako_terminal_view.h"

#include <QColor>
#include <QImage>
#include <QKeyEvent>
#include <QMouseEvent>
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

#include <chrono>
#include <cstdio>

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
    uint64_t atlas_generation;
};

void *tako_surface_new(uint16_t cols, uint16_t rows, const char *font_path,
                       uint32_t pixel_height);
void tako_surface_destroy(void *surface);
void tako_surface_tick(void *surface, TakoFramePlan *out);
void tako_surface_write(void *surface, const uint8_t *data, size_t len);
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
    // Accept left-button clicks so the item can claim active keyboard focus on
    // click; middle button is reserved for selection paste (TODO phase-1-§5).
    setAcceptedMouseButtons(Qt::LeftButton | Qt::MiddleButton);
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
    const auto t0 = std::chrono::steady_clock::now();
    ensureSurface();
    if (!m_surface) return oldNode;

    TakoFramePlan plan;
    const auto t_tick_start = std::chrono::steady_clock::now();
    tako_surface_tick(m_surface, &plan);
    const auto t_tick_end = std::chrono::steady_clock::now();

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

    // --- atlas texture (rebuild on content change OR dimension change) ---
    // The atlas pixels, glyph placements, and UVs are all recomputed in Rust
    // whenever a new glyph id appears. Dimensions may stay the same (shelf-pack
    // repacks into the existing canvas), so we additionally track a generation
    // counter bumped on every rebuild; without this, stale glyph placements
    // corrupt the render once the canvas stabilizes in size.
    const bool dims_changed =
        !m_atlasInit || (int)plan.atlas_w != m_atlasW || (int)plan.atlas_h != m_atlasH;
    bool atlas_texture_rebuilt = false;
    if (plan.atlas_w && plan.atlas_h &&
        (plan.atlas_generation != m_atlasGen || dims_changed)) {
        atlas_texture_rebuilt = true;
        const auto t_atlas_start = std::chrono::steady_clock::now();
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
        m_atlasGen = plan.atlas_generation;
        m_atlasInit = true;
        const auto t_atlas_end = std::chrono::steady_clock::now();
        const auto atlas_us =
            std::chrono::duration_cast<std::chrono::microseconds>(t_atlas_end - t_atlas_start)
                .count();
        if (atlas_us > 2000) {
            fprintf(stderr,
                    "[updatePaintNode] atlas texture rebuild=%lldµs (%ux%u)\n",
                    (long long)atlas_us, plan.atlas_w, plan.atlas_h);
        }
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

    const auto t_end = std::chrono::steady_clock::now();
    const auto total_us =
        std::chrono::duration_cast<std::chrono::microseconds>(t_end - t0).count();
    const auto tick_us =
        std::chrono::duration_cast<std::chrono::microseconds>(t_tick_end - t_tick_start).count();
    const auto qsg_us =
        std::chrono::duration_cast<std::chrono::microseconds>(t_end - t_tick_end).count();
    // Log slow frames: >5ms total OR atlas rebuild OR lots of quads. The
    // tick-vs-qsg split tells us whether the cost is in the Rust core or in
    // QSG node building / texture upload.
    if (total_us > 5000 || atlas_texture_rebuilt || plan.quad_count > 1500) {
        fprintf(stderr,
                "[updatePaintNode] total=%lldµs tick=%lldµs qsg=%lldµs "
                "atlas_rebuilt=%d quads=%zu atlas=%ux%u\n",
                (long long)total_us, (long long)tick_us, (long long)qsg_us,
                (int)atlas_texture_rebuilt, (size_t)plan.quad_count, plan.atlas_w,
                plan.atlas_h);
    }

    return root;
}

// ---- keyboard input ----
//
// Translate a QKeyEvent into the byte sequence a terminal application expects
// (ANSI/VT escape sequences for special keys, raw bytes for printable chars,
// and control codes for Ctrl+letter combos), then write them to the PTY via
// tako_surface_write. Modified keys (Shift+arrow etc.) currently send the
// unmodified sequence; full modifier-aware CSI (~ and 1;A-style) sequences are
// TODO(phase-1-§5) once we wire app-cursor / modifier reporting.

void TakoTerminalView::keyPressEvent(QKeyEvent *e) {
    if (!m_surface) {
        QQuickItem::keyPressEvent(e);
        return;
    }

    const int key = e->key();
    const Qt::KeyboardModifiers mods = e->modifiers();
    const QString text = e->text();
    QByteArray bytes;

    const bool ctrl = mods & Qt::ControlModifier;

    // Ctrl+A..Z → 0x01..0x1A (terminal control characters, e.g. Ctrl+C = ETX).
    if (ctrl && key >= Qt::Key_A && key <= Qt::Key_Z) {
        bytes.append(static_cast<char>(key - Qt::Key_A + 1));
    }
    // Ctrl+[ ] \ → ESC (0x1b), FS (0x1c), GS (0x1d). Ctrl+Space/@ → NUL.
    else if (ctrl && key == Qt::Key_BracketLeft) {
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
                // DEL (0x7f) is the conventional terminal backspace; ghostty
                // and most shells treat ^? as erase-char by default.
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
                // Printable: forward UTF-8. Text may be empty for unmapped
                // hardware keys; ignore those rather than spamming the shell.
                if (!text.isEmpty()) {
                    bytes.append(text.toUtf8());
                } else {
                    QQuickItem::keyPressEvent(e);
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

// ---- mouse ----
//
// For now a click only claims active focus so that subsequent key events are
// delivered to this item. Drag selection, middle-click paste, and SGR mouse
// reporting land in phase-1-§5.

void TakoTerminalView::mousePressEvent(QMouseEvent *e) {
    forceActiveFocus();
    e->accept();
}
