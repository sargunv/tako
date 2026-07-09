// TakoTerminalView + TakoTerminalRenderer implementation. See the header.
//
// The C ABI surface consumed here owns terminal session lifecycle + tick
// (produces a FramePlan).
//
// Threading model:
//   - TakoTerminalView lives on the GUI thread. Its QTimer calls
//     tako_terminal_session_tick to refresh m_plan, then QQuickItem::update() to
//     schedule a render.
//   - TakoTerminalRenderer::synchronize (GUI thread) copies the latest plan
//     into C++ renderer staging. The framework serializes synchronize with
//     render().
//   - TakoTerminalRenderer::render (render thread) lazily initializes GL
//     resources against Qt's current context and issues the draw.

#include "tako_terminal_view.h"

#include <QDesktopServices>
#include <QGuiApplication>
#include <QClipboard>
#include <QHoverEvent>
#include <QInputMethodEvent>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QOpenGLContext>
#include <QQuickWindow>
#include <QScreen>
#include <QSocketNotifier>
#include <QTimer>
#include <QUrl>
#include <QWheelEvent>
#include <QtQuick/qquickwindow.h>

#include <algorithm>
#include <chrono>
#include <climits>
#include <cmath>
#include <cstdio>

// libghostty-vt enum values used by the input C ABI. The headers are
// header-only (typedefs + enum constants), so we can include them here
// without linking against the library.
#include <ghostty/vt/key/event.h>
#include <ghostty/vt/mouse/event.h>
#pragma push_macro("emit")
#undef emit
#include <ghostty/vt/selection.h>
#pragma pop_macro("emit")

// The private terminal-core ABI comes from `tako_terminal_core.h`, pulled in
// via the header. FramePlan/Vertex are owned by tako_terminal_frame.h rather
// than generated from Rust.

// Register TakoTerminalView as `TerminalView` under a dedicated URI. Call once
// before loading QML through `tako_terminal::register_qml_types()`.
extern "C" void tako_register_qml_types() {
    qmlRegisterType<TakoTerminalView>("org.tako.terminal", 1, 0, "TerminalView");
}

namespace {
constexpr uint32_t COLOR_ROLE_FOREGROUND = 0;
constexpr uint32_t COLOR_ROLE_BACKGROUND = 1;
constexpr uint32_t COLOR_ROLE_CURSOR = 2;
constexpr qsizetype MAX_QUADS = 1 << 14;
constexpr qsizetype MAX_VERTICES = MAX_QUADS * 4;

const char *VERTEX_SHADER_SRC = R"(
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
uniform vec2 u_viewport;
varying vec2 v_uv;
varying vec4 v_color;
void main() {
    vec2 ndc = (a_pos / u_viewport) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, ndc.y, 0.0, 1.0);
    v_uv = a_uv;
    v_color = a_color;
}
)";

const char *FRAGMENT_SHADER_SRC = R"(
#ifdef GL_ES
precision mediump float;
#endif
uniform sampler2D u_atlas;
varying vec2 v_uv;
varying vec4 v_color;
void main() {
    float coverage = (v_uv.x < 0.0) ? 1.0 : texture2D(u_atlas, v_uv).r;
    gl_FragColor = vec4(v_color.rgb, v_color.a * coverage);
}
)";

int preedit_cursor_utf16(const QInputMethodEvent *event, const QString &preedit) {
    int cursor = preedit.size();
    for (const QInputMethodEvent::Attribute &attr : event->attributes()) {
        if (attr.type == QInputMethodEvent::Cursor) {
            cursor = std::clamp(attr.start, 0, static_cast<int>(preedit.size()));
            break;
        }
    }
    return cursor;
}

QString take_utf8(TakoTerminalBytes bytes) {
    QString text;
    if (bytes.ptr && bytes.len > 0) {
        text = QString::fromUtf8(
            reinterpret_cast<const char *>(bytes.ptr),
            static_cast<qsizetype>(bytes.len));
    }
    tako_terminal_bytes_free(bytes);
    return text;
}
}  // namespace

// ---- TakoTerminalView (GUI thread) ----

TakoTerminalView::TakoTerminalView(QQuickItem *parent)
    : QQuickFramebufferObject(parent) {
    // Click-to-focus; all buttons claimed so we can do middle-click paste,
    // right-click selection extend, etc.
    setAcceptedMouseButtons(Qt::AllButtons);
    setAcceptHoverEvents(true);
    setFlag(ItemAcceptsInputMethod, true);
    // The surface + readiness notifier are created in componentComplete(), on
    // the GUI thread, after QML has had a chance to set embeddable session
    // properties such as program and initialWorkingDirectory.
    // Safety + autorun timer. Output latency is handled by m_notifier (wired
    // in ensureSurface once the surface — and its readiness fd — exists); this
    // timer just keeps the autorun harness ticking and catches any wake the
    // notifier might miss at teardown edges.
    m_timer = new QTimer(this);
    m_timer->setInterval(100);
    connect(m_timer, &QTimer::timeout, this, [this] { pumpAndRender(); });
    m_timer->start();

    m_selectionAutoscrollTimer = new QTimer(this);
    m_selectionAutoscrollTimer->setInterval(50);
    connect(m_selectionAutoscrollTimer, &QTimer::timeout, this, [this] {
        if (!m_session) {
            stopSelectionAutoscroll();
            return;
        }
        const bool changed = tako_terminal_session_selection_autoscroll_tick(
            m_session, m_selectionMouseX, m_selectionMouseY, m_selectionMods) != 0;
        syncSelectionAutoscroll();
        if (changed) {
            pumpAndRender();
        }
    });

    m_cursorBlinkTimer = new QTimer(this);
    m_cursorBlinkTimer->setInterval(530);
    connect(m_cursorBlinkTimer, &QTimer::timeout, this, [this] {
        setCursorBlinkVisible(!m_cursorBlinkVisible);
    });
}

TakoTerminalView::~TakoTerminalView() {
    destroySurface();
}

void TakoTerminalView::componentComplete() {
    QQuickFramebufferObject::componentComplete();
    m_completed = true;
    if (m_autoStart) {
        start();
    }
}

void TakoTerminalView::setProgram(const QString &program) {
    if (m_program == program) return;
    m_program = program;
    emit programChanged();
}

void TakoTerminalView::setInitialWorkingDirectory(const QString &workingDirectory) {
    if (m_initialWorkingDirectory == workingDirectory) return;
    m_initialWorkingDirectory = workingDirectory;
    emit initialWorkingDirectoryChanged();
}

void TakoTerminalView::setScrollbackLimit(qulonglong rows) {
    const qulonglong clamped = std::max<qulonglong>(1, rows);
    if (m_scrollbackLimit == clamped) return;
    m_scrollbackLimit = clamped;
    emit scrollbackLimitChanged();
}

void TakoTerminalView::setShellIntegration(bool enabled) {
    if (m_shellIntegration == enabled) return;
    m_shellIntegration = enabled;
    emit shellIntegrationChanged();
}

void TakoTerminalView::setAutoStart(bool enabled) {
    if (m_autoStart == enabled) return;
    m_autoStart = enabled;
    emit autoStartChanged();
    if (m_autoStart && m_completed && !m_session) {
        start();
    }
}

void TakoTerminalView::setFontFamily(const QString &family) {
    if (m_fontFamily == family) return;
    m_fontFamily = family;
    emit fontFamilyChanged();
    applyFont();
}

void TakoTerminalView::setFontPixelSize(int pixelSize) {
    const int clamped = std::max(1, pixelSize);
    if (m_fontPixelSize == clamped) return;
    m_fontPixelSize = clamped;
    m_fontPointSize = static_cast<double>(m_fontPixelSize) * 72.0 / logicalDpi();
    emit fontPixelSizeChanged();
    emit fontPointSizeChanged();
    applyFont();
}

void TakoTerminalView::setFontPointSize(double pointSize) {
    const double clamped = std::max(1.0, pointSize);
    const int pixelSize =
        std::max(1, static_cast<int>(std::round(clamped * logicalDpi() / 72.0)));
    if (std::abs(m_fontPointSize - clamped) < 0.01 &&
        m_fontPixelSize == pixelSize) {
        return;
    }
    const bool pixelChanged = m_fontPixelSize != pixelSize;
    m_fontPointSize = clamped;
    m_fontPixelSize = pixelSize;
    emit fontPointSizeChanged();
    if (pixelChanged) emit fontPixelSizeChanged();
    applyFont();
}

void TakoTerminalView::setForegroundColor(const QColor &color) {
    const QColor normalized = color.isValid() ? color.toRgb() : QColor();
    if (m_foregroundColor == normalized) return;
    m_foregroundColor = normalized;
    emit colorsChanged();
    applyColors();
}

void TakoTerminalView::resetForegroundColor() {
    setForegroundColor(QColor());
}

void TakoTerminalView::setBackgroundColor(const QColor &color) {
    const QColor normalized = color.isValid() ? color.toRgb() : QColor();
    if (m_backgroundColor == normalized) return;
    m_backgroundColor = normalized;
    emit colorsChanged();
    applyColors();
}

void TakoTerminalView::resetBackgroundColor() {
    setBackgroundColor(QColor());
}

void TakoTerminalView::setCursorColor(const QColor &color) {
    const QColor normalized = color.isValid() ? color.toRgb() : QColor();
    if (m_cursorColor == normalized) return;
    m_cursorColor = normalized;
    emit colorsChanged();
    applyColors();
}

void TakoTerminalView::resetCursorColor() {
    setCursorColor(QColor());
}

void TakoTerminalView::setColorPalette(const QVariantList &palette) {
    QVariantList normalized;
    if (!palette.isEmpty()) {
        if (palette.size() != 256) return;
        normalized.reserve(256);
        for (const QVariant &entry : palette) {
            QColor color = entry.value<QColor>();
            if (!color.isValid()) {
                color = QColor(entry.toString());
            }
            if (!color.isValid()) return;
            normalized.push_back(color.toRgb());
        }
    }

    if (m_colorPalette == normalized) return;
    m_colorPalette = normalized;
    emit colorsChanged();
    applyColors();
}

void TakoTerminalView::resetColorPalette() {
    setColorPalette(QVariantList());
}

void TakoTerminalView::setCursorStyle(CursorStyle style) {
    if (style < BarCursor || style > HollowBlockCursor) {
        style = BlockCursor;
    }
    if (m_cursorStyle == style) return;
    m_cursorStyle = style;
    emit cursorSettingsChanged();
    applyCursorSettings();
}

void TakoTerminalView::setCursorBlink(bool blink) {
    if (m_cursorBlink == blink) return;
    m_cursorBlink = blink;
    emit cursorSettingsChanged();
    applyCursorSettings();
}

void TakoTerminalView::setSingleClickSelection(SelectionUnit unit) {
    if (unit < CellSelection || unit > CommandOutputSelection) {
        unit = CellSelection;
    }
    if (m_singleClickSelection == unit) return;
    m_singleClickSelection = unit;
    emit selectionBehaviorChanged();
}

void TakoTerminalView::setDoubleClickSelection(SelectionUnit unit) {
    if (unit < CellSelection || unit > CommandOutputSelection) {
        unit = WordSelection;
    }
    if (m_doubleClickSelection == unit) return;
    m_doubleClickSelection = unit;
    emit selectionBehaviorChanged();
}

void TakoTerminalView::setTripleClickSelection(SelectionUnit unit) {
    if (unit < CellSelection || unit > CommandOutputSelection) {
        unit = LineSelection;
    }
    if (m_tripleClickSelection == unit) return;
    m_tripleClickSelection = unit;
    emit selectionBehaviorChanged();
}

QString TakoTerminalView::engineVersion() const {
    char buf[256];
    const size_t n = tako_terminal_core_engine_version(
        reinterpret_cast<uint8_t *>(buf), sizeof(buf) - 1);
    if (n == 0) return {};
    buf[n] = '\0';
    return QString::fromUtf8(buf, static_cast<int>(n));
}

void TakoTerminalView::copySelection() {
    if (!m_session) return;
    const QString text =
        take_utf8(tako_terminal_session_selection_text_owned(m_session));
    if (text.isEmpty()) return;
    if (auto *clip = QGuiApplication::clipboard()) {
        clip->setText(text, QClipboard::Clipboard);
    }
}

void TakoTerminalView::pasteClipboard() {
    if (!m_session) return;
    if (auto *clip = QGuiApplication::clipboard()) {
        const QString text = clip->text(QClipboard::Clipboard);
        if (!text.isEmpty()) {
            const QByteArray bytes = text.toUtf8();
            tako_terminal_session_paste(
                m_session,
                reinterpret_cast<const uint8_t *>(bytes.constData()),
                static_cast<size_t>(bytes.size()));
        }
    }
}

void TakoTerminalView::clearSelection() {
    if (!m_session) return;
    stopSelectionAutoscroll();
    tako_terminal_session_selection_clear(m_session);
    pumpAndRender();
}

bool TakoTerminalView::selectAll() {
    if (!m_session) return false;
    stopSelectionAutoscroll();
    const bool selected = tako_terminal_session_selection_all(m_session) != 0;
    if (selected) pumpAndRender();
    return selected;
}

bool TakoTerminalView::selectCommandOutputAt(double x, double y) {
    if (!m_session) return false;
    stopSelectionAutoscroll();
    const float dpr = windowDpr();
    const bool selected = tako_terminal_session_selection_output_at(
        m_session,
        static_cast<float>(x) * dpr,
        static_cast<float>(y) * dpr) != 0;
    if (selected) pumpAndRender();
    return selected;
}

bool TakoTerminalView::selectCommandInputAt(double x, double y) {
    if (!m_session) return false;
    stopSelectionAutoscroll();
    const float dpr = windowDpr();
    const bool selected = tako_terminal_session_selection_input_at(
        m_session,
        static_cast<float>(x) * dpr,
        static_cast<float>(y) * dpr) != 0;
    if (selected) pumpAndRender();
    return selected;
}

void TakoTerminalView::writeText(const QString &text) {
    if (!m_session || text.isEmpty()) return;
    const QByteArray bytes = text.toUtf8();
    tako_terminal_session_write(
        m_session,
        reinterpret_cast<const uint8_t *>(bytes.constData()),
        static_cast<size_t>(bytes.size()));
}

void TakoTerminalView::scrollLines(int lines) {
    if (!m_session || lines == 0) return;
    tako_terminal_session_scroll(m_session, static_cast<int64_t>(lines));
    pumpAndRender();
}

void TakoTerminalView::scrollToTop() {
    if (!m_session) return;
    tako_terminal_session_scroll_to_top(m_session);
    pumpAndRender();
}

void TakoTerminalView::scrollToBottom() {
    if (!m_session) return;
    tako_terminal_session_scroll_to_bottom(m_session);
    pumpAndRender();
}

void TakoTerminalView::scrollToRow(qulonglong row) {
    if (!m_session) return;
    tako_terminal_session_scroll_to_row(m_session, static_cast<uint64_t>(row));
    pumpAndRender();
}

void TakoTerminalView::start() {
    if (m_session || !m_completed) return;
    ensureSurface();
    pumpAndRender();
}

void TakoTerminalView::stop() {
    if (!m_session) return;
    stopSelectionAutoscroll();
    destroySurface();
    m_plan = {};
    m_title.clear();
    emit titleChanged();
    m_currentWorkingDirectory.clear();
    emit currentWorkingDirectoryChanged();
    m_hoveredHyperlink.clear();
    emit hoveredHyperlinkChanged();
    m_scrollbarTotal = 0;
    m_scrollbarOffset = 0;
    m_scrollbarLength = 0;
    m_viewportAtBottom = true;
    emit scrollbarChanged();
    m_exited = false;
    emit exitedChanged();
    if (m_runningNotified) {
        m_runningNotified = false;
        emit runningChanged();
    }
    update();
}

void TakoTerminalView::restart() {
    stop();
    start();
}

void TakoTerminalView::pumpAndRender() {
    if (!m_session) return;
    // Clear any pending wake bytes so the level-triggered notifier settles,
    // then advance the terminal. tick() reports whether a new frame was
    // actually produced; if not, skip update() and the GPU stays idle.
    // (Sizing is event-driven via geometryChange + onDprChanged — no polling
    // here. A DPR change forces a replan inside tick even if the Zig-owned grid
    // size is unchanged, so the GL viewport refreshes.)
    tako_terminal_session_drain_notify(m_session);
    const bool changed = tako_terminal_session_tick(m_session, &m_plan);
    flushHostState();
    flushScrollbarState();
    flushBell();
    if (changed) {
        update();
    }
    if (!m_exited && tako_terminal_session_exited(m_session)) {
        m_exited = true;
        emit exitedChanged();
        if (m_runningNotified) {
            m_runningNotified = false;
            emit runningChanged();
        }
        emit exited();
    }
}

void TakoTerminalView::syncSelectionAutoscroll() {
    if (!m_session || !m_selectionAutoscrollTimer) return;
    const int direction =
        tako_terminal_session_selection_autoscroll(m_session);
    if (direction == 0) {
        stopSelectionAutoscroll();
    } else if (!m_selectionAutoscrollTimer->isActive()) {
        m_selectionAutoscrollTimer->start();
    }
}

void TakoTerminalView::stopSelectionAutoscroll() {
    if (m_selectionAutoscrollTimer && m_selectionAutoscrollTimer->isActive()) {
        m_selectionAutoscrollTimer->stop();
    }
}

void TakoTerminalView::startCursorBlink() {
    if (!m_session || !hasActiveFocus()) return;
    setCursorBlinkVisible(true);
    if (m_cursorBlinkTimer && !m_cursorBlinkTimer->isActive()) {
        m_cursorBlinkTimer->start();
    }
}

void TakoTerminalView::stopCursorBlink(bool repaint) {
    if (m_cursorBlinkTimer && m_cursorBlinkTimer->isActive()) {
        m_cursorBlinkTimer->stop();
    }
    m_cursorBlinkVisible = true;
    if (m_session) {
        tako_terminal_session_set_cursor_blink_visible(m_session, true);
        if (repaint) pumpAndRender();
    }
}

void TakoTerminalView::setCursorBlinkVisible(bool visible) {
    if (!m_session) {
        m_cursorBlinkVisible = visible;
        return;
    }
    if (m_cursorBlinkVisible == visible) return;
    m_cursorBlinkVisible = visible;
    tako_terminal_session_set_cursor_blink_visible(m_session, visible);
    pumpAndRender();
}

QString TakoTerminalView::hyperlinkAt(const QPointF &position) const {
    if (!m_session) return {};
    const float dpr = windowDpr();
    char buf[4096];
    const size_t n = tako_terminal_session_hyperlink_at(
        m_session,
        static_cast<float>(position.x()) * dpr,
        static_cast<float>(position.y()) * dpr,
        reinterpret_cast<uint8_t *>(buf),
        sizeof(buf) - 1);
    if (n == 0) return {};
    buf[n] = '\0';
    return QString::fromUtf8(buf, static_cast<int>(n));
}

QRectF TakoTerminalView::cursorRectangle() const {
    const float dpr = windowDpr();
    if (dpr <= 0.0f || m_plan.cell_w <= 0.0f || m_plan.cell_h <= 0.0f ||
        m_plan.cursor_present == 0) {
        return QRectF();
    }

    return QRectF(
        static_cast<qreal>(m_plan.cursor_x * m_plan.cell_w / dpr),
        static_cast<qreal>(m_plan.cursor_y * m_plan.cell_h / dpr),
        static_cast<qreal>(m_plan.cell_w / dpr),
        static_cast<qreal>(m_plan.cell_h / dpr));
}

bool TakoTerminalView::hasHyperlinkOpenModifier(Qt::KeyboardModifiers modifiers) const {
    return modifiers.testFlag(Qt::ControlModifier) || modifiers.testFlag(Qt::MetaModifier);
}

void TakoTerminalView::updateHoveredHyperlink(const QPointF &position,
                                              Qt::KeyboardModifiers modifiers) {
    const QString next =
        hasHyperlinkOpenModifier(modifiers) ? hyperlinkAt(position) : QString{};
    if (next != m_hoveredHyperlink) {
        m_hoveredHyperlink = next;
        emit hoveredHyperlinkChanged();
    }
    if (m_hoveredHyperlink.isEmpty()) {
        unsetCursor();
    } else {
        setCursor(Qt::PointingHandCursor);
    }
}

float TakoTerminalView::windowDpr() const {
    if (auto *w = window()) {
        return static_cast<float>(w->devicePixelRatio());
    }
    return 1.0f;
}

double TakoTerminalView::logicalDpi() const {
    if (auto *w = window()) {
        if (auto *screen = w->screen()) {
            return screen->logicalDotsPerInch();
        }
    }
    if (auto *screen = QGuiApplication::primaryScreen()) {
        return screen->logicalDotsPerInch();
    }
    return 96.0;
}

void TakoTerminalView::applyFont() {
    if (!m_session) return;
    stopSelectionAutoscroll();
    const QByteArray family = m_fontFamily.toUtf8();
    const int ok = tako_terminal_session_set_font(
        m_session,
        nullptr,
        family.isEmpty() ? nullptr : family.constData(),
        static_cast<uint32_t>(m_fontPixelSize));
    if (!ok) return;
    const float dpr = windowDpr();
    if (width() >= 1.0 && height() >= 1.0) {
        tako_terminal_session_resize_pixels(
            m_session,
            static_cast<uint32_t>(width() * dpr),
            static_cast<uint32_t>(height() * dpr));
    }
    pumpAndRender();
}

void TakoTerminalView::applyColors() {
    if (!m_session) return;
    auto apply = [this](uint32_t role, const QColor &color) {
        const bool enabled = color.isValid();
        tako_terminal_session_set_default_color(
            m_session,
            role,
            enabled,
            enabled ? static_cast<uint8_t>(color.red()) : 0,
            enabled ? static_cast<uint8_t>(color.green()) : 0,
            enabled ? static_cast<uint8_t>(color.blue()) : 0);
    };
    apply(COLOR_ROLE_FOREGROUND, m_foregroundColor);
    apply(COLOR_ROLE_BACKGROUND, m_backgroundColor);
    apply(COLOR_ROLE_CURSOR, m_cursorColor);

    if (m_colorPalette.size() == 256) {
        QByteArray palette;
        palette.resize(256 * 3);
        for (qsizetype i = 0; i < m_colorPalette.size(); ++i) {
            const QColor color = m_colorPalette.at(i).value<QColor>().toRgb();
            const qsizetype base = i * 3;
            palette[base] = static_cast<char>(color.red());
            palette[base + 1] = static_cast<char>(color.green());
            palette[base + 2] = static_cast<char>(color.blue());
        }
        tako_terminal_session_set_default_palette(
            m_session,
            true,
            reinterpret_cast<const uint8_t *>(palette.constData()),
            static_cast<uintptr_t>(palette.size()));
    } else {
        tako_terminal_session_set_default_palette(m_session, false, nullptr, 0);
    }
    pumpAndRender();
}

void TakoTerminalView::applyCursorSettings() {
    if (!m_session) return;
    if (tako_terminal_session_set_default_cursor(
            m_session,
            static_cast<uint32_t>(m_cursorStyle),
            m_cursorBlink) != 0) {
        if (m_cursorBlink) {
            startCursorBlink();
        } else {
            stopCursorBlink(true);
        }
    }
    pumpAndRender();
}

void TakoTerminalView::flushScrollbarState() {
    if (!m_session) return;

    TakoTerminalScrollbarState state = {};
    if (!tako_terminal_session_scrollbar_state(m_session, &state)) {
        return;
    }

    const auto total = static_cast<qulonglong>(state.total);
    const auto offset = static_cast<qulonglong>(state.offset);
    const auto length = static_cast<qulonglong>(state.len);
    const bool atBottom = state.viewport_active != 0;
    if (m_scrollbarTotal == total &&
        m_scrollbarOffset == offset &&
        m_scrollbarLength == length &&
        m_viewportAtBottom == atBottom) {
        return;
    }

    m_scrollbarTotal = total;
    m_scrollbarOffset = offset;
    m_scrollbarLength = length;
    m_viewportAtBottom = atBottom;
    emit scrollbarChanged();
}

void TakoTerminalView::flushBell() {
    if (!m_session) return;
    const uint32_t count = tako_terminal_session_take_bell_count(m_session);
    if (count == 0) return;
    emit bell(static_cast<int>(std::min<uint32_t>(count, INT_MAX)));
}

void TakoTerminalView::onDprChanged() {
    if (!m_session) return;
    const float dpr = windowDpr();
    tako_terminal_session_set_dpr(m_session, dpr);
    // Reflow to the new cell metrics. Cell metrics are now physical, so pass
    // the item's physical size (DIP × DPR).
    if (width() >= 1.0 && height() >= 1.0) {
        tako_terminal_session_resize_pixels(m_session,
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
    if (!m_completed) return;
    if (!m_session) {
        // Placeholder grid; resized to the item's actual geometry immediately
        // below (or on the first geometryChange if the item isn't laid out
        // yet). fontPixelSize is the LOGICAL cell height; the surface
        // multiplies it by the window's devicePixelRatio to rasterize at
        // physical resolution.
        const float dpr = windowDpr();
        const QByteArray program = m_program.toUtf8();
        const QByteArray cwd = m_initialWorkingDirectory.toUtf8();
        const QByteArray fontFamily = m_fontFamily.toUtf8();
        TakoTerminalOptions options = {};
        options.cols = 80;
        options.rows = 24;
        options.font_path = nullptr;
        options.font_family = fontFamily.isEmpty() ? nullptr : fontFamily.constData();
        options.pixel_height = static_cast<uint32_t>(m_fontPixelSize);
        options.dpr = dpr;
        options.program =
            program.isEmpty() ? nullptr : program.constData();
        options.working_directory =
            cwd.isEmpty() ? nullptr : cwd.constData();
        options.max_scrollback = static_cast<uintptr_t>(m_scrollbackLimit);
        options.shell_integration = m_shellIntegration;
        m_session = tako_terminal_session_new(&options);
        if (m_session && !m_runningNotified) {
            m_runningNotified = true;
            emit runningChanged();
        }
        // Resize to the item's actual physical size on creation so the grid
        // matches the window from the first frame (cell metrics are physical
        // post-P4, so pass width × dpr).
        if (m_session && width() >= 1.0 && height() >= 1.0) {
            const uint32_t phys_w = static_cast<uint32_t>(width() * dpr);
            const uint32_t phys_h = static_cast<uint32_t>(height() * dpr);
            tako_terminal_session_resize_pixels(m_session, phys_w, phys_h);
        }
        if (m_session) {
            m_cursorBlinkVisible = true;
            tako_terminal_session_set_focused(m_session, hasActiveFocus());
            tako_terminal_session_set_cursor_blink_visible(m_session, true);
            applyColors();
            applyCursorSettings();
        }
        // Wire the PTY master fd: wake immediately on output instead of
        // waiting for the safety timer. fd == -1 means timer-only fallback.
        if (m_session) {
            const int fd = tako_terminal_session_notify_fd(m_session);
            if (fd >= 0) {
                m_notifier = new QSocketNotifier(fd, QSocketNotifier::Read, this);
                connect(m_notifier, &QSocketNotifier::activated, this,
                        [this] { pumpAndRender(); });
            }
        }
    }
}

void TakoTerminalView::destroySurface() {
    stopSelectionAutoscroll();
    stopCursorBlink(false);
    // Stop watching the readiness fd before the surface (which owns it) is
    // freed — otherwise the notifier would dangle.
    if (m_notifier) {
        m_notifier->setEnabled(false);
        delete m_notifier;
        m_notifier = nullptr;
    }
    if (m_session) {
        tako_terminal_session_destroy(m_session);
        m_session = nullptr;
    }
}

void TakoTerminalView::geometryChange(const QRectF &newGeometry,
                                      const QRectF &oldGeometry) {
    QQuickFramebufferObject::geometryChange(newGeometry, oldGeometry);
    ensureSurface();
    if (!m_session) return;
    // Skip degenerate sizes (0×0 at startup, or during teardown).
    if (newGeometry.width() < 1.0 || newGeometry.height() < 1.0) {
        return;
    }
    // `newGeometry` is in DIPs; backend cell metrics are physical, so pass
    // physical px (DIP × DPR). Zig applies a terminal resize only when the
    // computed grid changes, so sub-cell motion during drag stays cheap.
    const float dpr = windowDpr();
    tako_terminal_session_resize_pixels(
        m_session, static_cast<uint32_t>(newGeometry.width() * dpr),
        static_cast<uint32_t>(newGeometry.height() * dpr));
}

QQuickFramebufferObject::Renderer *TakoTerminalView::createRenderer() const {
    // The terminal session is GUI-thread state; this runs on the QSG render
    // thread and only spawns the render-thread renderer wrapper.
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

bool selection_adjustment_for_qt_key(int key, uint32_t *out) {
    if (!out) return false;
    switch (key) {
        case Qt::Key_Left:
            *out = GHOSTTY_SELECTION_ADJUST_LEFT;
            return true;
        case Qt::Key_Right:
            *out = GHOSTTY_SELECTION_ADJUST_RIGHT;
            return true;
        case Qt::Key_Up:
            *out = GHOSTTY_SELECTION_ADJUST_UP;
            return true;
        case Qt::Key_Down:
            *out = GHOSTTY_SELECTION_ADJUST_DOWN;
            return true;
        case Qt::Key_Home:
            *out = GHOSTTY_SELECTION_ADJUST_BEGINNING_OF_LINE;
            return true;
        case Qt::Key_End:
            *out = GHOSTTY_SELECTION_ADJUST_END_OF_LINE;
            return true;
        case Qt::Key_PageUp:
            *out = GHOSTTY_SELECTION_ADJUST_PAGE_UP;
            return true;
        case Qt::Key_PageDown:
            *out = GHOSTTY_SELECTION_ADJUST_PAGE_DOWN;
            return true;
        default:
            return false;
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

void TakoTerminalView::flushHostState() {
    if (!m_session) return;
    char buf[512];
    size_t n = tako_terminal_session_take_title(
        m_session, reinterpret_cast<uint8_t *>(buf), sizeof(buf) - 1);
    if (n > 0) {
        buf[n] = '\0';
        const QString next = QString::fromUtf8(buf, static_cast<int>(n));
        if (next != m_title) {
            m_title = next;
            emit titleChanged();
        }
    }
    char cwd[4096];
    n = tako_terminal_session_take_pwd(
        m_session, reinterpret_cast<uint8_t *>(cwd), sizeof(cwd) - 1);
    if (n > 0) {
        cwd[n] = '\0';
        const QString next = QString::fromUtf8(cwd, static_cast<int>(n));
        if (next != m_currentWorkingDirectory) {
            m_currentWorkingDirectory = next;
            emit currentWorkingDirectoryChanged();
        }
    }
}

void TakoTerminalView::mousePressEvent(QMouseEvent *e) {
    forceActiveFocus();
    if (!m_session) { e->accept(); return; }

    const bool mouse_tracking =
        tako_terminal_session_mouse_tracking(m_session) != 0;

    if (!mouse_tracking && e->button() == Qt::LeftButton &&
        hasHyperlinkOpenModifier(e->modifiers())) {
        const QString uri = hyperlinkAt(e->position());
        if (!uri.isEmpty()) {
            stopSelectionAutoscroll();
            clearSelection();
            QDesktopServices::openUrl(QUrl::fromUserInput(uri));
            e->accept();
            return;
        }
    }

    if (mouse_tracking) {
        stopSelectionAutoscroll();
        // Program wants raw events: forward to the encoder.
        const float dpr = windowDpr();
        const QPointF p = e->position();
        tako_terminal_session_mouse_event(
            m_session, mouse_action_press(), qt_button_to_ghostty(e->button()),
            static_cast<float>(p.x()) * dpr,
            static_cast<float>(p.y()) * dpr,
            qt_mods_to_ghostty(e->modifiers()));
        m_anyMouseButtonHeld = true;
        tako_terminal_session_mouse_set_any_button(m_session, true);
    } else if (e->button() == Qt::MiddleButton) {
        // Middle-click paste from the PRIMARY selection (X11 convention).
        if (const auto *clip = QGuiApplication::clipboard()) {
            const QString text = clip->text(QClipboard::Selection);
            if (!text.isEmpty()) {
                const QByteArray bytes = text.toUtf8();
                tako_terminal_session_paste(
                    m_session,
                    reinterpret_cast<const uint8_t *>(bytes.constData()),
                    static_cast<size_t>(bytes.size()));
            }
        }
    } else {
        // Selection gesture: begin (press). time_ns drives multi-click counting.
        const float dpr = windowDpr();
        const QPointF p = e->position();
        m_selectionMouseX = static_cast<float>(p.x()) * dpr;
        m_selectionMouseY = static_cast<float>(p.y()) * dpr;
        m_selectionMods = qt_mods_to_ghostty(e->modifiers());
        const auto time_ns = static_cast<uint64_t>(
            std::chrono::steady_clock::now().time_since_epoch().count());
        tako_terminal_session_selection_begin(
            m_session,
            m_selectionMouseX,
            m_selectionMouseY,
            time_ns, m_selectionMods,
            static_cast<uint32_t>(m_singleClickSelection),
            static_cast<uint32_t>(m_doubleClickSelection),
            static_cast<uint32_t>(m_tripleClickSelection));
        syncSelectionAutoscroll();
        pumpAndRender();
    }
    e->accept();
}

void TakoTerminalView::mouseReleaseEvent(QMouseEvent *e) {
    if (!m_session) { e->accept(); return; }

    const bool mouse_tracking =
        tako_terminal_session_mouse_tracking(m_session) != 0;
    if (mouse_tracking) {
        stopSelectionAutoscroll();
        const float dpr = windowDpr();
        const QPointF p = e->position();
        tako_terminal_session_mouse_event(
            m_session, mouse_action_release(),
            qt_button_to_ghostty(e->button()),
            static_cast<float>(p.x()) * dpr,
            static_cast<float>(p.y()) * dpr,
            qt_mods_to_ghostty(e->modifiers()));
    } else if (e->button() == Qt::LeftButton) {
        stopSelectionAutoscroll();
        // Finalize the selection. Copy the result to the PRIMARY selection
        // (X11 middle-click paste convention) — copy-on-select.
        const float dpr = windowDpr();
        const QPointF p = e->position();
        const QString text = take_utf8(tako_terminal_session_selection_end_owned(
            m_session,
            static_cast<float>(p.x()) * dpr,
            static_cast<float>(p.y()) * dpr));
        if (!text.isEmpty()) {
            if (auto *clip = QGuiApplication::clipboard()) {
                clip->setText(text, QClipboard::Selection);
            }
        }
        pumpAndRender();
    }
    // Any-button tracking state: recompute from current app mouse buttons.
    const bool any_held =
        (QGuiApplication::mouseButtons() != Qt::NoButton);
    if (any_held != m_anyMouseButtonHeld) {
        m_anyMouseButtonHeld = any_held;
        tako_terminal_session_mouse_set_any_button(m_session, any_held);
    }
    e->accept();
}

void TakoTerminalView::mouseMoveEvent(QMouseEvent *e) {
    if (!m_session) { e->accept(); return; }
    const bool mouse_tracking =
        tako_terminal_session_mouse_tracking(m_session) != 0;
    if (mouse_tracking) {
        updateHoveredHyperlink(e->position(), {});
        stopSelectionAutoscroll();
        const float dpr = windowDpr();
        const QPointF p = e->position();
        tako_terminal_session_mouse_event(
            m_session, mouse_action_motion(), /*button=*/0,
            static_cast<float>(p.x()) * dpr,
            static_cast<float>(p.y()) * dpr,
            qt_mods_to_ghostty(e->modifiers()));
    } else if (e->buttons() & Qt::LeftButton) {
        updateHoveredHyperlink(e->position(), {});
        // Drag-extend the selection while the left button is held. Gated on
        // the live button state (not m_anyMouseButtonHeld, which tracks the
        // mouse-encoder's any-button flag and is only set in the tracking-on
        // branch).
        const float dpr = windowDpr();
        const QPointF p = e->position();
        m_selectionMouseX = static_cast<float>(p.x()) * dpr;
        m_selectionMouseY = static_cast<float>(p.y()) * dpr;
        m_selectionMods = qt_mods_to_ghostty(e->modifiers());
        if (tako_terminal_session_selection_extend(
                m_session, m_selectionMouseX, m_selectionMouseY,
                m_selectionMods)) {
            pumpAndRender();
        }
        syncSelectionAutoscroll();
    } else {
        updateHoveredHyperlink(e->position(), e->modifiers());
    }
    e->accept();
}

void TakoTerminalView::hoverMoveEvent(QHoverEvent *e) {
    if (m_session) {
        updateHoveredHyperlink(e->position(), e->modifiers());
    }
    e->accept();
}

void TakoTerminalView::hoverLeaveEvent(QHoverEvent *e) {
    updateHoveredHyperlink({}, {});
    e->accept();
}

void TakoTerminalView::wheelEvent(QWheelEvent *e) {
    if (!m_session) { e->accept(); return; }

    // Mouse-wheel scrolling: when mouse tracking is on, encode as button-4/5
    // events. Otherwise, scroll the local viewport directly (alternate-screen
    // applications like less/vim enable mode 1007 / mouse tracking, so this
    // branch is reached only when the program is *not* capturing the wheel).
    const bool mouse_tracking =
        tako_terminal_session_mouse_tracking(m_session) != 0;
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
            tako_terminal_session_mouse_event(
                m_session, mouse_action_press(), btn,
                static_cast<float>(p.x()) * dpr,
                static_cast<float>(p.y()) * dpr,
                qt_mods_to_ghostty(e->modifiers()));
            tako_terminal_session_mouse_event(
                m_session, mouse_action_release(), btn,
                static_cast<float>(p.x()) * dpr,
                static_cast<float>(p.y()) * dpr,
                qt_mods_to_ghostty(e->modifiers()));
        }
        if (vsteps != 0) {
            pumpAndRender();
        }
    } else {
        // Scroll the viewport by lines. ±15 degrees ≈ one notch = 3 lines
        // (xterm default).
        const int lines = static_cast<int>(std::round(deg.y() / 5.0));
        if (lines != 0) {
            scrollLines(-lines);
        }
    }
    e->accept();
}

void TakoTerminalView::inputMethodEvent(QInputMethodEvent *e) {
    if (!m_session) {
        e->accept();
        return;
    }

    const QString commit = e->commitString();
    if (!commit.isEmpty()) {
        const QByteArray bytes = commit.toUtf8();
        tako_terminal_session_write(
            m_session,
            reinterpret_cast<const uint8_t *>(bytes.constData()),
            static_cast<size_t>(bytes.size()));
    }

    const QString preedit = e->preeditString();
    const QByteArray preeditBytes = preedit.toUtf8();
    const int cursorUtf16 = preedit_cursor_utf16(e, preedit);
    const QByteArray cursorPrefix = preedit.left(cursorUtf16).toUtf8();
    tako_terminal_session_set_preedit(
        m_session,
        reinterpret_cast<const uint8_t *>(preeditBytes.constData()),
        static_cast<size_t>(preeditBytes.size()),
        static_cast<size_t>(cursorPrefix.size()));
    pumpAndRender();
    e->accept();
}

QVariant TakoTerminalView::inputMethodQuery(Qt::InputMethodQuery query) const {
    switch (query) {
        case Qt::ImEnabled:
            return true;
        case Qt::ImCursorRectangle:
            return cursorRectangle();
        default:
            return QQuickFramebufferObject::inputMethodQuery(query);
    }
}

void TakoTerminalView::focusInEvent(QFocusEvent *e) {
    if (m_session) tako_terminal_session_focus_event(m_session, /*gained=*/true);
    if (m_cursorBlink) {
        startCursorBlink();
    } else {
        stopCursorBlink(false);
    }
    pumpAndRender();
    e->accept();
}

void TakoTerminalView::focusOutEvent(QFocusEvent *e) {
    if (m_session) tako_terminal_session_focus_event(m_session, /*gained=*/false);
    stopCursorBlink(true);
    updateHoveredHyperlink({}, {});
    e->accept();
}

// ---- keyboard input ----
//
// Translate QKeyEvent to GhosttyKey + mods and hand off to the Zig-owned
// libghostty-vt key encoder (DEC modes, modifyOtherKeys, Kitty keyboard, etc.).

void TakoTerminalView::keyPressEvent(QKeyEvent *e) {
    if (!m_session) {
        QQuickFramebufferObject::keyPressEvent(e);
        return;
    }

    // Terminal copy/paste shortcuts (Ctrl+Shift+C / Ctrl+Shift+V) — handled
    // before the key encoder so they never reach the shell.
    if ((e->modifiers() & (Qt::ControlModifier | Qt::ShiftModifier)) ==
            (Qt::ControlModifier | Qt::ShiftModifier) &&
        (e->key() == Qt::Key_C || e->key() == Qt::Key_V)) {
        if (auto *clip = QGuiApplication::clipboard()) {
            if (e->key() == Qt::Key_C) {
                copySelection();
            } else {
                const QString text = clip->text(QClipboard::Clipboard);
                if (!text.isEmpty()) {
                    pasteClipboard();
                }
            }
        }
        e->accept();
        return;
    }

    const auto selection_mods =
        Qt::ShiftModifier | Qt::ControlModifier | Qt::AltModifier |
        Qt::MetaModifier;
    if ((e->modifiers() & selection_mods) == Qt::ShiftModifier) {
        uint32_t adjustment = 0;
        if (selection_adjustment_for_qt_key(e->key(), &adjustment)) {
            m_keyboardSelectionKey = e->key();
            if (tako_terminal_session_selection_adjust(m_session, adjustment)) {
                pumpAndRender();
            }
            e->accept();
            return;
        }
    }

    const GhosttyKey key = qt_key_to_ghostty(e->key());
    if (key == GHOSTTY_KEY_UNIDENTIFIED) {
        // Fallback: if the event carries printable text, send it raw.
        const QString text = e->text();
        if (!text.isEmpty()) {
            const QByteArray bytes = text.toUtf8();
            tako_terminal_session_write(
                m_session,
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

    tako_terminal_session_key_event(m_session, action, static_cast<uint32_t>(key),
                           mods, consumed_mods, text_ptr, text_len);
    e->accept();
}

void TakoTerminalView::keyReleaseEvent(QKeyEvent *e) {
    if (!m_session) {
        QQuickFramebufferObject::keyReleaseEvent(e);
        return;
    }
    // Qt sometimes emits autorepeat on release; libghostty-vt wants a true
    // release only on the final key-up.
    if (e->isAutoRepeat()) {
        e->accept();
        return;
    }
    uint32_t ignored_adjustment = 0;
    if (m_keyboardSelectionKey == e->key() &&
        selection_adjustment_for_qt_key(e->key(), &ignored_adjustment)) {
        m_keyboardSelectionKey = 0;
        e->accept();
        return;
    }
    const GhosttyKey key = qt_key_to_ghostty(e->key());
    if (key == GHOSTTY_KEY_UNIDENTIFIED) {
        e->accept();
        return;
    }
    const uint16_t mods = qt_mods_to_ghostty(e->modifiers());
    tako_terminal_session_key_event(m_session, GHOSTTY_KEY_ACTION_RELEASE,
                           static_cast<uint32_t>(key), mods, 0,
                           nullptr, 0);
    e->accept();
}

// ---- TakoTerminalRenderer (render thread) ----
//
// Owns the GL pipeline directly on Qt's render thread.

TakoTerminalRenderer::TakoTerminalRenderer() = default;

TakoTerminalRenderer::~TakoTerminalRenderer() {
    // GL resources belong to the current Qt context. Destruction is usually on
    // the render thread, but Qt does not guarantee the context is current here;
    // leak-to-context-lifetime rather than deleting against the wrong context.
}

void TakoTerminalRenderer::synchronize(QQuickFramebufferObject *item) {
    auto *view = static_cast<TakoTerminalView *>(item);
    // Physical-pixel viewport: item logical size × window DPR. The FBO Qt
    // creates for us is sized in physical pixels (textureFollowsItemSize).
    const QSizeF logical = view->size();
    const qreal dpr = view->window() ? view->window()->devicePixelRatio() : 1.0;
    const int vw = static_cast<int>(logical.width() * dpr);
    const int vh = static_cast<int>(logical.height() * dpr);
    m_viewportW = std::max(1, vw);
    m_viewportH = std::max(1, vh);

    const FramePlan &plan = view->plan();
    m_clearColor = {
        plan.clear_color[0],
        plan.clear_color[1],
        plan.clear_color[2],
        plan.clear_color[3],
    };

    m_vertices.clear();
    if (plan.vertices && plan.vertex_count > 0) {
        const uintptr_t take = std::min<uintptr_t>(plan.vertex_count, MAX_VERTICES);
        m_vertices.insert(m_vertices.end(), plan.vertices, plan.vertices + take);
    }

    if (plan.atlas_generation != m_atlasGeneration && plan.atlas_pixels &&
        plan.atlas_w > 0 && plan.atlas_h > 0) {
        const uintptr_t size =
            static_cast<uintptr_t>(plan.atlas_w) * static_cast<uintptr_t>(plan.atlas_h);
        m_atlasPixels.assign(plan.atlas_pixels, plan.atlas_pixels + size);
        m_atlasW = plan.atlas_w;
        m_atlasH = plan.atlas_h;
        m_atlasGeneration = plan.atlas_generation;
        m_atlasDirty = true;
    }
}

void TakoTerminalRenderer::render() {
    ensureGl();
    if (!m_gl) {
        return;
    }

    if (m_atlasDirty && !m_atlasPixels.empty() && m_atlasW > 0 && m_atlasH > 0) {
        m_gl->glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
        m_gl->glBindTexture(GL_TEXTURE_2D, m_atlasTexture);
        m_gl->glTexImage2D(GL_TEXTURE_2D, 0, GL_LUMINANCE,
                           static_cast<GLsizei>(m_atlasW),
                           static_cast<GLsizei>(m_atlasH), 0,
                           GL_LUMINANCE, GL_UNSIGNED_BYTE, m_atlasPixels.data());
        m_atlasDirty = false;
    }

    m_gl->glViewport(0, 0, m_viewportW, m_viewportH);
    m_gl->glClearColor(m_clearColor[0] / 255.0f, m_clearColor[1] / 255.0f,
                       m_clearColor[2] / 255.0f, m_clearColor[3] / 255.0f);
    m_gl->glClear(GL_COLOR_BUFFER_BIT);

    if (m_vertices.empty()) {
        return;
    }

    m_gl->glEnable(GL_BLEND);
    m_gl->glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    m_gl->glBindBuffer(GL_ARRAY_BUFFER, m_vbo);
    m_gl->glBufferSubData(GL_ARRAY_BUFFER, 0,
                          static_cast<GLsizeiptr>(m_vertices.size() * sizeof(Vertex)),
                          m_vertices.data());

    m_gl->glUseProgram(m_program);
    m_gl->glUniform2f(m_uViewport, static_cast<GLfloat>(m_viewportW),
                      static_cast<GLfloat>(m_viewportH));
    m_gl->glActiveTexture(GL_TEXTURE0);
    m_gl->glBindTexture(GL_TEXTURE_2D, m_atlasTexture);

    m_gl->glBindBuffer(GL_ARRAY_BUFFER, m_vbo);
    m_gl->glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, m_ibo);
    m_gl->glEnableVertexAttribArray(0);
    m_gl->glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, sizeof(Vertex),
                                reinterpret_cast<const void *>(0));
    m_gl->glEnableVertexAttribArray(1);
    m_gl->glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, sizeof(Vertex),
                                reinterpret_cast<const void *>(sizeof(float) * 2));
    m_gl->glEnableVertexAttribArray(2);
    m_gl->glVertexAttribPointer(2, 4, GL_UNSIGNED_BYTE, GL_TRUE, sizeof(Vertex),
                                reinterpret_cast<const void *>(sizeof(float) * 4));

    const GLsizei indexCount = static_cast<GLsizei>((m_vertices.size() / 4) * 6);
    m_gl->glDrawElements(GL_TRIANGLES, indexCount, GL_UNSIGNED_INT, nullptr);

    m_gl->glDisableVertexAttribArray(0);
    m_gl->glDisableVertexAttribArray(1);
    m_gl->glDisableVertexAttribArray(2);
    m_gl->glUseProgram(0);
}

void TakoTerminalRenderer::ensureGl() {
    if (m_glInited) {
        return;
    }
    auto *ctx = QOpenGLContext::currentContext();
    if (!ctx) {
        return;
    }
    m_gl = ctx->functions();
    m_gl->initializeOpenGLFunctions();

    m_program = linkProgram();

    m_gl->glGenBuffers(1, &m_vbo);
    m_gl->glBindBuffer(GL_ARRAY_BUFFER, m_vbo);
    m_gl->glBufferData(GL_ARRAY_BUFFER,
                       static_cast<GLsizeiptr>(MAX_VERTICES * sizeof(Vertex)),
                       nullptr, GL_DYNAMIC_DRAW);

    std::vector<uint32_t> indices;
    indices.reserve(MAX_QUADS * 6);
    for (uint32_t i = 0; i < static_cast<uint32_t>(MAX_QUADS); ++i) {
        const uint32_t b = i * 4;
        indices.insert(indices.end(), {b, b + 1, b + 2, b, b + 2, b + 3});
    }
    m_gl->glGenBuffers(1, &m_ibo);
    m_gl->glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, m_ibo);
    m_gl->glBufferData(GL_ELEMENT_ARRAY_BUFFER,
                       static_cast<GLsizeiptr>(indices.size() * sizeof(uint32_t)),
                       indices.data(), GL_STATIC_DRAW);

    m_gl->glGenTextures(1, &m_atlasTexture);
    m_gl->glBindTexture(GL_TEXTURE_2D, m_atlasTexture);
    m_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    m_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    m_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    m_gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);

    m_uViewport = m_gl->glGetUniformLocation(m_program, "u_viewport");
    const GLint uAtlas = m_gl->glGetUniformLocation(m_program, "u_atlas");
    m_gl->glUseProgram(m_program);
    m_gl->glUniform1i(uAtlas, 0);
    m_gl->glUseProgram(0);

    m_glInited = true;
}

GLuint TakoTerminalRenderer::compileShader(GLenum type, const char *source) {
    const GLuint shader = m_gl->glCreateShader(type);
    m_gl->glShaderSource(shader, 1, &source, nullptr);
    m_gl->glCompileShader(shader);
    GLint ok = GL_FALSE;
    m_gl->glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[1024] = {};
        m_gl->glGetShaderInfoLog(shader, sizeof(log), nullptr, log);
        qFatal("terminal shader compile failed: %s", log);
    }
    return shader;
}

GLuint TakoTerminalRenderer::linkProgram() {
    const GLuint vs = compileShader(GL_VERTEX_SHADER, VERTEX_SHADER_SRC);
    const GLuint fs = compileShader(GL_FRAGMENT_SHADER, FRAGMENT_SHADER_SRC);
    const GLuint program = m_gl->glCreateProgram();
    m_gl->glAttachShader(program, vs);
    m_gl->glAttachShader(program, fs);
    m_gl->glBindAttribLocation(program, 0, "a_pos");
    m_gl->glBindAttribLocation(program, 1, "a_uv");
    m_gl->glBindAttribLocation(program, 2, "a_color");
    m_gl->glLinkProgram(program);
    GLint ok = GL_FALSE;
    m_gl->glGetProgramiv(program, GL_LINK_STATUS, &ok);
    if (!ok) {
        char log[1024] = {};
        m_gl->glGetProgramInfoLog(program, sizeof(log), nullptr, log);
        qFatal("terminal shader link failed: %s", log);
    }
    m_gl->glDetachShader(program, vs);
    m_gl->glDetachShader(program, fs);
    m_gl->glDeleteShader(vs);
    m_gl->glDeleteShader(fs);
    return program;
}
