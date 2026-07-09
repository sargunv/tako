// TakoTerminalView: an embeddable Qt Quick terminal component backed by
// libghostty-vt. Owns the terminal session on the GUI thread (woken by a
// QSocketNotifier on the PTY master fd, plus a safety timer); the
// render-thread `TakoTerminalRenderer` draws directly into the Qt-provided FBO.
// See the .cpp for the design.
//
// The Qt facade calls only the private Zig ABI in `tako_terminal_core.h`.
// FramePlan/Vertex live in `tako_terminal_frame.h`; implementation-only
// snapshot/font structs remain private to Zig.

#pragma once

#include <QtQuick/qquickframebufferobject.h>

#include <cstddef>
#include <cstdint>

#include <QColor>
#include <QOpenGLFunctions>
#include <QPointF>
#include <QRectF>
#include <QString>
#include <QVariant>

#include <array>
#include <vector>

#include "tako_terminal_core.h"

class QInputMethodEvent;
class QKeyEvent;
class QMouseEvent;
class QHoverEvent;
class QWheelEvent;
class QFocusEvent;
class QTimer;
class QSocketNotifier;

class TakoTerminalRenderer;

class TakoTerminalView : public QQuickFramebufferObject {
    Q_OBJECT
    Q_PROPERTY(QString program READ program WRITE setProgram NOTIFY programChanged)
    Q_PROPERTY(QString initialWorkingDirectory READ initialWorkingDirectory WRITE setInitialWorkingDirectory NOTIFY initialWorkingDirectoryChanged)
    Q_PROPERTY(qulonglong scrollbackLimit READ scrollbackLimit WRITE setScrollbackLimit NOTIFY scrollbackLimitChanged)
    Q_PROPERTY(bool shellIntegration READ shellIntegration WRITE setShellIntegration NOTIFY shellIntegrationChanged)
    Q_PROPERTY(bool autoStart READ autoStart WRITE setAutoStart NOTIFY autoStartChanged)
    Q_PROPERTY(bool running READ isRunning NOTIFY runningChanged)
    Q_PROPERTY(QString title READ title NOTIFY titleChanged)
    Q_PROPERTY(QString currentWorkingDirectory READ currentWorkingDirectory NOTIFY currentWorkingDirectoryChanged)
    Q_PROPERTY(QString hoveredHyperlink READ hoveredHyperlink NOTIFY hoveredHyperlinkChanged)
    Q_PROPERTY(bool exited READ hasExited NOTIFY exitedChanged)
    Q_PROPERTY(qulonglong scrollbarTotal READ scrollbarTotal NOTIFY scrollbarChanged)
    Q_PROPERTY(qulonglong scrollbarOffset READ scrollbarOffset NOTIFY scrollbarChanged)
    Q_PROPERTY(qulonglong scrollbarLength READ scrollbarLength NOTIFY scrollbarChanged)
    Q_PROPERTY(bool viewportAtBottom READ viewportAtBottom NOTIFY scrollbarChanged)
    Q_PROPERTY(QString fontFamily READ fontFamily WRITE setFontFamily NOTIFY fontFamilyChanged)
    Q_PROPERTY(int fontPixelSize READ fontPixelSize WRITE setFontPixelSize NOTIFY fontPixelSizeChanged)
    Q_PROPERTY(double fontPointSize READ fontPointSize WRITE setFontPointSize NOTIFY fontPointSizeChanged)
    Q_PROPERTY(QColor foregroundColor READ foregroundColor WRITE setForegroundColor RESET resetForegroundColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor backgroundColor READ backgroundColor WRITE setBackgroundColor RESET resetBackgroundColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor cursorColor READ cursorColor WRITE setCursorColor RESET resetCursorColor NOTIFY colorsChanged)
    Q_PROPERTY(QVariantList colorPalette READ colorPalette WRITE setColorPalette RESET resetColorPalette NOTIFY colorsChanged)
    Q_PROPERTY(CursorStyle cursorStyle READ cursorStyle WRITE setCursorStyle NOTIFY cursorSettingsChanged)
    Q_PROPERTY(bool cursorBlink READ cursorBlink WRITE setCursorBlink NOTIFY cursorSettingsChanged)
    Q_PROPERTY(SelectionUnit singleClickSelection READ singleClickSelection WRITE setSingleClickSelection NOTIFY selectionBehaviorChanged)
    Q_PROPERTY(SelectionUnit doubleClickSelection READ doubleClickSelection WRITE setDoubleClickSelection NOTIFY selectionBehaviorChanged)
    Q_PROPERTY(SelectionUnit tripleClickSelection READ tripleClickSelection WRITE setTripleClickSelection NOTIFY selectionBehaviorChanged)
    Q_PROPERTY(QString engineVersion READ engineVersion CONSTANT)

public:
    enum SelectionUnit {
        CellSelection = 0,
        WordSelection = 1,
        LineSelection = 2,
        CommandOutputSelection = 3,
    };
    Q_ENUM(SelectionUnit)

    enum CursorStyle {
        BarCursor = 0,
        BlockCursor = 1,
        UnderlineCursor = 2,
        HollowBlockCursor = 3,
    };
    Q_ENUM(CursorStyle)

    explicit TakoTerminalView(QQuickItem *parent = nullptr);
    ~TakoTerminalView() override;

    QQuickFramebufferObject::Renderer *createRenderer() const override;

    QString program() const { return m_program; }
    void setProgram(const QString &program);

    QString initialWorkingDirectory() const { return m_initialWorkingDirectory; }
    void setInitialWorkingDirectory(const QString &workingDirectory);
    qulonglong scrollbackLimit() const { return m_scrollbackLimit; }
    void setScrollbackLimit(qulonglong rows);
    bool shellIntegration() const { return m_shellIntegration; }
    void setShellIntegration(bool enabled);
    bool autoStart() const { return m_autoStart; }
    void setAutoStart(bool enabled);

    QString title() const { return m_title; }
    QString currentWorkingDirectory() const { return m_currentWorkingDirectory; }
    QString hoveredHyperlink() const { return m_hoveredHyperlink; }
    bool hasExited() const { return m_exited; }
    bool isRunning() const { return m_session && !m_exited; }
    qulonglong scrollbarTotal() const { return m_scrollbarTotal; }
    qulonglong scrollbarOffset() const { return m_scrollbarOffset; }
    qulonglong scrollbarLength() const { return m_scrollbarLength; }
    bool viewportAtBottom() const { return m_viewportAtBottom; }

    QString fontFamily() const { return m_fontFamily; }
    void setFontFamily(const QString &family);
    int fontPixelSize() const { return m_fontPixelSize; }
    void setFontPixelSize(int pixelSize);
    double fontPointSize() const { return m_fontPointSize; }
    void setFontPointSize(double pointSize);
    QColor foregroundColor() const { return m_foregroundColor; }
    void setForegroundColor(const QColor &color);
    void resetForegroundColor();
    QColor backgroundColor() const { return m_backgroundColor; }
    void setBackgroundColor(const QColor &color);
    void resetBackgroundColor();
    QColor cursorColor() const { return m_cursorColor; }
    void setCursorColor(const QColor &color);
    void resetCursorColor();
    QVariantList colorPalette() const { return m_colorPalette; }
    void setColorPalette(const QVariantList &palette);
    void resetColorPalette();
    CursorStyle cursorStyle() const { return m_cursorStyle; }
    void setCursorStyle(CursorStyle style);
    bool cursorBlink() const { return m_cursorBlink; }
    void setCursorBlink(bool blink);
    SelectionUnit singleClickSelection() const { return m_singleClickSelection; }
    void setSingleClickSelection(SelectionUnit unit);
    SelectionUnit doubleClickSelection() const { return m_doubleClickSelection; }
    void setDoubleClickSelection(SelectionUnit unit);
    SelectionUnit tripleClickSelection() const { return m_tripleClickSelection; }
    void setTripleClickSelection(SelectionUnit unit);
    QString engineVersion() const;

    Q_INVOKABLE void copySelection();
    Q_INVOKABLE void pasteClipboard();
    Q_INVOKABLE void clearSelection();
    Q_INVOKABLE bool selectAll();
    Q_INVOKABLE bool selectCommandOutputAt(double x, double y);
    Q_INVOKABLE bool selectCommandInputAt(double x, double y);
    Q_INVOKABLE void writeText(const QString &text);
    Q_INVOKABLE void scrollLines(int lines);
    Q_INVOKABLE void scrollToTop();
    Q_INVOKABLE void scrollToBottom();
    Q_INVOKABLE void scrollToRow(qulonglong row);
    Q_INVOKABLE void start();
    Q_INVOKABLE void stop();
    Q_INVOKABLE void restart();

    // Latest FramePlan copy from the Zig session. Borrowed buffers inside it
    // remain valid until the next rebuilt frame or session stop.
    const FramePlan &plan() const { return m_plan; }

signals:
    // Emitted once when the hosted PTY session exits. Embedders decide whether
    // to close this view, show restart UI, or quit the application.
    void exited();
    void exitedChanged();
    void programChanged();
    void initialWorkingDirectoryChanged();
    void scrollbackLimitChanged();
    void shellIntegrationChanged();
    void autoStartChanged();
    void runningChanged();
    void titleChanged();
    void currentWorkingDirectoryChanged();
    void hoveredHyperlinkChanged();
    void scrollbarChanged();
    void fontFamilyChanged();
    void fontPixelSizeChanged();
    void fontPointSizeChanged();
    void colorsChanged();
    void cursorSettingsChanged();
    void selectionBehaviorChanged();
    void bell(int count);

protected:
    // Keyboard: extract QKeyEvent data and forward to the Zig-owned
    // libghostty-vt key encoder.
    void keyPressEvent(QKeyEvent *e) override;
    void keyReleaseEvent(QKeyEvent *e) override;

    // Mouse: route to the encoder when mouse tracking is on; otherwise handle
    // selection, wheel-scroll, hyperlinks, and middle-click paste.
    void mousePressEvent(QMouseEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;
    void mouseMoveEvent(QMouseEvent *e) override;
    void hoverMoveEvent(QHoverEvent *e) override;
    void hoverLeaveEvent(QHoverEvent *e) override;
    void wheelEvent(QWheelEvent *e) override;
    void inputMethodEvent(QInputMethodEvent *e) override;
    QVariant inputMethodQuery(Qt::InputMethodQuery query) const override;
    void focusInEvent(QFocusEvent *e) override;
    void focusOutEvent(QFocusEvent *e) override;
    void componentComplete() override;

    // Resize: pass item pixels to the implementation core; Zig owns cols/rows
    // and uses backend cell metrics to reflow the terminal.
    void geometryChange(const QRectF &newGeometry, const QRectF &oldGeometry) override;

    // Detect when the item joins a window so we can wire DPR-change signals.
    void itemChange(ItemChange change, const ItemChangeData &value) override;

private:
    void ensureSurface();
    void destroySurface();
    // Read the window's current device-pixel ratio. Falls back to 1.0 when the
    // item isn't in a window yet (e.g. during construction).
    float windowDpr() const;
    // React to a DPR change (window moved between monitors, or the screen's
    // DPR changed): reload the font at the new physical size and reflow.
    void onDprChanged();
    // Apply font-family/size changes to the live surface without restarting
    // the PTY, then reflow to the current item geometry.
    void applyFont();
    // Apply default theme/cursor configuration to libghostty-vt. Colors are
    // terminal defaults; OSC overrides remain owned by libghostty state.
    void applyColors();
    void applyCursorSettings();
    double logicalDpi() const;
    // Pull fresh host-visible title/cwd values from the surface and emit
    // property notifications.
    void flushHostState();
    // Pull fresh scrollback viewport state from the implementation core for
    // embedder-owned UI such as scrollbars.
    void flushScrollbarState();
    // Drain BEL events from the implementation core and emit them as Qt
    // notifications for embedders to handle.
    void flushBell();
    // Tick the implementation core; `update()` only if it produced a new
    // frame. Driven by both the PTY wake notifier and the safety timer.
    void pumpAndRender();
    // Start/stop the selection autoscroll timer according to the active
    // libghostty-vt gesture state.
    void syncSelectionAutoscroll();
    void stopSelectionAutoscroll();
    void startCursorBlink();
    void stopCursorBlink(bool repaint);
    void setCursorBlinkVisible(bool visible);
    QString hyperlinkAt(const QPointF &position) const;
    QRectF cursorRectangle() const;
    bool hasHyperlinkOpenModifier(Qt::KeyboardModifiers modifiers) const;
    void updateHoveredHyperlink(const QPointF &position, Qt::KeyboardModifiers modifiers);

    TakoTerminalSession *m_session = nullptr;
    QTimer *m_timer = nullptr;
    QTimer *m_selectionAutoscrollTimer = nullptr;
    QTimer *m_cursorBlinkTimer = nullptr;
    QSocketNotifier *m_notifier = nullptr;
    FramePlan m_plan = {};
    bool m_dprSignalConnected = false;
    bool m_completed = false;
    bool m_exited = false;
    bool m_runningNotified = false;
    bool m_cursorBlinkVisible = true;
    // Tracks whether any mouse button is held, for any-event motion reporting.
    bool m_anyMouseButtonHeld = false;
    int m_keyboardSelectionKey = 0;
    float m_selectionMouseX = 0.0f;
    float m_selectionMouseY = 0.0f;
    uint16_t m_selectionMods = 0;
    QString m_program;
    QString m_initialWorkingDirectory;
    QString m_title;
    QString m_currentWorkingDirectory;
    QString m_hoveredHyperlink;
    SelectionUnit m_singleClickSelection = CellSelection;
    SelectionUnit m_doubleClickSelection = WordSelection;
    SelectionUnit m_tripleClickSelection = LineSelection;
    qulonglong m_scrollbarTotal = 0;
    qulonglong m_scrollbarOffset = 0;
    qulonglong m_scrollbarLength = 0;
    qulonglong m_scrollbackLimit = 10000;
    bool m_shellIntegration = true;
    bool m_autoStart = true;
    bool m_viewportAtBottom = true;
    QString m_fontFamily;
    int m_fontPixelSize = 18;
    double m_fontPointSize = 13.5;
    QColor m_foregroundColor;
    QColor m_backgroundColor;
    QColor m_cursorColor;
    QVariantList m_colorPalette;
    CursorStyle m_cursorStyle = BlockCursor;
    bool m_cursorBlink = false;
};

// Render-thread renderer. C++ owns GL resources because the Qt renderer object
// already lives on the render thread with the correct OpenGL context current.
class TakoTerminalRenderer : public QQuickFramebufferObject::Renderer {
public:
    TakoTerminalRenderer();
    ~TakoTerminalRenderer() override;

    void synchronize(QQuickFramebufferObject *item) override;
    void render() override;

private:
    void ensureGl();
    GLuint compileShader(GLenum type, const char *source);
    GLuint linkProgram();

    QOpenGLFunctions *m_gl = nullptr;
    bool m_glInited = false;
    GLuint m_program = 0;
    GLuint m_vbo = 0;
    GLuint m_ibo = 0;
    GLuint m_atlasTexture = 0;
    GLint m_uViewport = -1;
    uint64_t m_atlasGeneration = 0;
    bool m_atlasDirty = false;
    std::vector<Vertex> m_vertices;
    std::vector<uint8_t> m_atlasPixels;
    uint32_t m_atlasW = 0;
    uint32_t m_atlasH = 0;
    std::array<uint8_t, 4> m_clearColor = {0, 0, 0, 255};
    int m_viewportW = 1;
    int m_viewportH = 1;
};
