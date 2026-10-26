"""Floating drag-out tray for finished downloads.

Runs as its own windowless process (``pythonw -m siphon_host.dropzone``) so a
Qt event loop never shares the native messaging host. The host writes one JSON
payload to stdin: ``{"items": [{"files", "title", "subtitle"}], "pinned",
"ffmpeg"}``. A second launch hands its payload to the running tray over a
local socket and exits, so finished downloads stack in one place.

Started with ``"resident": true`` (by the host at connect), the process
stays warm while the host lives. That makes ``"grab"`` possible: the
extension popup reports a drag gesture, and the tray takes over the held
mouse button with a card under the cursor, so the drag that continues is a
real file drag instead of the browser's.

Interaction: each card counts down and slides away. Hovering pauses every
countdown; a click pins a card (click again to release it); the close button
or a right-click dismisses it. Dragging a card drops its files into any app
that accepts Explorer drags, such as FL Studio's playlist or channel rack.
"""

import ctypes
import json
import logging
import os
import math
import subprocess
import sys
import threading
import time
from pathlib import Path

from PySide6.QtCore import QByteArray, QMimeData, QObject, QPointF, QRectF, Qt, QTimer, QUrl, Signal
from PySide6.QtGui import (QColor, QCursor, QDrag, QFont, QFontMetricsF, QGuiApplication, QLinearGradient,
                           QPainter, QPainterPath, QPen, QPixmap)
from PySide6.QtNetwork import QLocalServer, QLocalSocket
from PySide6.QtWidgets import QApplication, QWidget

from .runtime import tray_pipe_name

SERVER = tray_pipe_name()
log = logging.getLogger("siphon.tray")
LIFETIME = 7.0          # seconds a card stays without interaction
RESUME_MINIMUM = 1.6    # leaving the tray never dismisses a card instantly
MAX_CARDS = 5
CARD_W, CARD_H, GAP, MARGIN = 392., 66., 10., 28.
RADIUS = 18.

# Siphon's dark glass tokens (extension/src/shared/ui/tokens.css): neutral
# surfaces, with colour only from the card's service.
SURFACE = QColor(22, 22, 26, 244)
RAISED = QColor(34, 34, 39, 250)
BORDER = QColor(255, 255, 255, 22)
SPECULAR = QColor(255, 255, 255, 20)
TEXT = QColor("#f4f4f5")
MUTED = QColor("#a1a1aa")
ACCENTS = {"youtube": "#ff3355", "youtubeMusic": "#ff3355", "soundcloud": "#ff6a1a", "bandcamp": "#3cb8d8"}
NEUTRAL = "#f4f4f5"
FONT_FAMILY = "Segoe UI Variable Text"


def load_font():
    """Use the extension's Geist when bundled; fall back to Segoe."""
    global FONT_FAMILY
    from PySide6.QtGui import QFontDatabase
    face = QFontDatabase.addApplicationFont(str(Path(__file__).with_name("fonts") / "Geist-Variable.ttf"))
    families = QFontDatabase.applicationFontFamilies(face) if face >= 0 else []
    if families:
        FONT_FAMILY = families[0]


def animations_enabled():
    """Honor Windows' "Animation effects" accessibility setting."""
    if sys.platform != "win32":
        return True
    value = ctypes.c_bool(True)
    try:
        ctypes.windll.user32.SystemParametersInfoW(0x1042, 0, ctypes.byref(value), 0)  # SPI_GETCLIENTAREAANIMATION
    except OSError:
        return True
    return bool(value.value)


class Spring:
    """Critically-tuned spring; gives motion its weight and gentle settle."""

    def __init__(self, value=0., stiffness=420., damping=32.):
        self.value = self.target = value
        self.velocity = 0.
        self.stiffness, self.damping = stiffness, damping

    def snap(self, value):
        self.value = self.target = value
        self.velocity = 0.

    def step(self, dt):
        if self.value == self.target and not self.velocity:
            return False
        for _ in range(4):
            h = dt / 4
            self.velocity += (self.stiffness * (self.target - self.value) - self.damping * self.velocity) * h
            self.value += self.velocity * h
        if abs(self.target - self.value) < 5e-4 and abs(self.velocity) < 5e-3:
            self.snap(self.target)
            return False
        return True


def peaks(ffmpeg, path, bars=56):
    """Normalised loudness envelope for the card's waveform, or None."""
    import numpy as np
    rate = 2000
    try:
        result = subprocess.run([ffmpeg or "ffmpeg", "-v", "error", "-nostdin", "-i", str(path), "-vn", "-map", "0:a:0",
                                 "-ac", "1", "-ar", str(rate), "-f", "f32le", "-"], capture_output=True, timeout=60,
                                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    except (OSError, subprocess.TimeoutExpired):
        return None
    samples = np.abs(np.frombuffer(result.stdout[:len(result.stdout) // 4 * 4], dtype="<f4"))
    if samples.size < bars:
        return None
    edges = np.linspace(0, samples.size, bars + 1).astype(int)
    levels = np.array([np.sqrt(np.mean(samples[a:b] ** 2)) for a, b in zip(edges[:-1], edges[1:])])
    top = float(np.percentile(levels, 98)) or 1.
    return [float(min(1., v / top)) ** .8 for v in levels]


class Card:
    def __init__(self, item, pinned):
        self.files = [str(Path(f)) for f in item["files"]]
        self.title = item.get("title") or Path(self.files[0]).stem
        self.accent = QColor(ACCENTS.get(item.get("service"), NEUTRAL))
        self.subtitle = item.get("subtitle") or (Path(self.files[0]).suffix[1:].upper() if len(self.files) == 1 else f"{len(self.files)} files")
        self.pinned = pinned
        self.remaining = LIFETIME
        self.dropped_at = None
        self.peaks = None
        self.closing = False
        self.presence = Spring(0., 300., 26.)   # 0 hidden → 1 shown; slight overshoot
        self.y = Spring(0., 380., 34.)
        self.hover = Spring(0., 600., 46.)
        self.press = Spring(0., 900., 56.)
        self.pin = Spring(1. if pinned else 0., 520., 30.)
        self.flash = Spring(0., 260., 30.)       # drop confirmation
        self.reveal = Spring(0., 160., 26.)      # waveform grows in once measured

    @property
    def springs(self):
        return (self.presence, self.y, self.hover, self.press, self.pin, self.flash, self.reveal)


class Bridge(QObject):
    measured = Signal(object, object)


class Tray(QWidget):
    def __init__(self, ffmpeg):
        super().__init__(None, Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Tool | Qt.WindowDoesNotAcceptFocus
                         | Qt.NoDropShadowWindowHint)
        self.setAttribute(Qt.WA_TranslucentBackground)
        self.setAttribute(Qt.WA_ShowWithoutActivating)
        self.setMouseTracking(True)
        self.setWindowTitle("Siphon")
        self.ffmpeg = ffmpeg
        self.resident = False
        self.grabber = None
        self.cards = []
        self.hovered = None
        self.pressed = None
        self.press_pos = None
        self.dragging = False
        self.animate = animations_enabled()
        self.bridge = Bridge()
        self.bridge.measured.connect(self._measured)
        self.clock = time.monotonic()
        self.timer = QTimer(self, timerType=Qt.PreciseTimer, interval=8)
        self.timer.timeout.connect(self._tick)
        self.title_font = QFont(FONT_FAMILY, 10)
        self.title_font.setWeight(QFont.DemiBold)
        self.title_font.setLetterSpacing(QFont.PercentageSpacing, 99)
        self.sub_font = QFont(FONT_FAMILY, 8)

    # Layout -------------------------------------------------------------
    def _place(self):
        screen = QGuiApplication.screenAt(QCursor.pos()) or QGuiApplication.primaryScreen()
        area = screen.availableGeometry()
        live = max(1, sum(not c.closing for c in self.cards), len(self.cards))
        width = CARD_W + 2 * MARGIN
        height = live * (CARD_H + GAP) - GAP + 2 * MARGIN
        self.setGeometry(int(area.center().x() - width / 2), int(area.bottom() - 18 - height + MARGIN * .4), int(width), int(height))

    def _targets(self):
        live = [c for c in self.cards if not c.closing]
        for index, card in enumerate(reversed(live)):  # newest sits at the bottom, nearest the taskbar
            card.y.target = index

    def card_rect(self, card):
        scale = .94 + .06 * card.presence.value - .015 * card.press.value
        lift = (1 - card.presence.value) * 22 - card.hover.value * 1.5
        bottom = self.height() - MARGIN - card.y.value * (CARD_H + GAP)
        rect = QRectF(MARGIN, bottom - CARD_H, CARD_W, CARD_H).translated(0, lift)
        center = rect.center()
        rect.setSize(rect.size() * scale)
        rect.moveCenter(center)
        return rect

    def card_at(self, point):
        for card in reversed(self.cards):
            if not card.closing and self.card_rect(card).contains(point):
                return card
        return None

    def close_rect(self, card):
        rect = self.card_rect(card)
        return QRectF(rect.right() - 34, rect.top() + 9, 24, 24)

    # Payloads -----------------------------------------------------------
    def accept(self, payload):
        self.ffmpeg = payload.get("ffmpeg") or self.ffmpeg
        pinned = bool(payload.get("pinned"))
        for item in payload.get("items") or []:
            files = [f for f in item.get("files") or [] if isinstance(f, str) and Path(f).is_file()]
            if not files:
                continue
            if item.get("replaces"):
                # Late BPM/key for a card already on screen: refresh it quietly,
                # and never bring back a card the user has dismissed.
                replaced = [str(Path(f)) for f in item["replaces"]]
                card = next((c for c in self.cards if c.files == replaced and not c.closing), None)
                if card:
                    card.files = [str(Path(f)) for f in files]
                    card.title = item.get("title") or card.title
                    card.subtitle = item.get("subtitle") or card.subtitle
                    self.update()
                continue
            existing = next((c for c in self.cards if c.files == [str(Path(f)) for f in files] and not c.closing), None)
            if existing:
                # Re-summoning a visible card refreshes it with a small pulse.
                existing.remaining, existing.pinned = LIFETIME, existing.pinned or pinned
                existing.pin.target = 1. if existing.pinned else 0.
                existing.press.velocity = 9.
                self.cards.remove(existing)
                self.cards.append(existing)
                continue
            card = Card(dict(item, files=files), pinned)
            self.cards.append(card)
            threading.Thread(target=self._measure, args=(card,), daemon=True).start()
        live = [c for c in self.cards if not c.closing]
        for card in live[:max(0, len(live) - MAX_CARDS)]:
            self.dismiss(card)
        self._targets()
        for card in self.cards:
            if card.presence.target == 0 and not card.closing:
                card.y.snap(card.y.target)
                card.presence.target = 1.
        if not self.cards:
            return
        self._place()
        if not self.isVisible():
            self.show()
        self._start()

    def _measure(self, card):
        self.bridge.measured.emit(card, peaks(self.ffmpeg, card.files[0]))

    def _measured(self, card, values):
        if values:
            card.peaks = values
            card.reveal.target = 1.
            self._start()

    def dismiss(self, card):
        if card.closing:
            return
        card.closing = True
        card.presence.stiffness, card.presence.damping = 520., 40.
        card.presence.target = 0.
        self._targets()
        self._start()

    # Animation loop -----------------------------------------------------
    def _start(self):
        self.clock = time.monotonic()
        if not self.timer.isActive():
            self.timer.start()

    def _tick(self):
        now = time.monotonic()
        dt = min(1 / 30, now - self.clock)
        self.clock = now
        busy = False
        paused = self.hovered is not None or self.dragging
        for card in list(self.cards):
            if not card.closing and not card.pinned and not paused:
                card.remaining -= dt
                if card.remaining <= 0:
                    self.dismiss(card)
            if card.dropped_at and not card.pinned and now - card.dropped_at > .9:
                self.dismiss(card)
            if card.closing or (not card.pinned and not paused) or card.dropped_at:
                busy = True
            card.hover.target = 1. if card is self.hovered else 0.
            card.press.target = 1. if card is self.pressed and not self.dragging else 0.
            for spring in card.springs:
                if self.animate:
                    busy |= spring.step(dt)
                else:
                    spring.snap(spring.target)
            if card.closing and card.presence.value <= .02:
                self.cards.remove(card)
                if self.hovered is card:
                    self.hovered = None
        self.update()
        if not self.cards:
            self.timer.stop()
            self.hide()
            if not self.resident:
                QApplication.quit()
        elif not busy:
            self.timer.stop()

    # Input --------------------------------------------------------------
    def mouseMoveEvent(self, event):
        point = event.position()
        if self.pressed and self.press_pos is not None and not self.dragging:
            if (point - self.press_pos).manhattanLength() >= QApplication.startDragDistance():
                self._drag(self.pressed)
                return
        hovered = self.card_at(point)
        if hovered is None and self.hovered is not None:
            for card in self.cards:
                card.remaining = max(card.remaining, RESUME_MINIMUM)
        self.hovered = hovered
        if not self.pressed:
            self.setCursor(Qt.ArrowCursor if not hovered else Qt.PointingHandCursor if self.close_rect(hovered).contains(point) else Qt.OpenHandCursor)
        self._start()

    def leaveEvent(self, event):
        if self.hovered is not None:
            for card in self.cards:
                card.remaining = max(card.remaining, RESUME_MINIMUM)
        self.hovered = None
        self._start()

    def mousePressEvent(self, event):
        card = self.card_at(event.position())
        if not card:
            return
        if event.button() == Qt.RightButton:
            self.dismiss(card)
            return
        if event.button() == Qt.LeftButton:
            self.pressed, self.press_pos = card, event.position()
            self.setCursor(Qt.ClosedHandCursor)
            self._start()

    def mouseReleaseEvent(self, event):
        card, self.pressed, self.press_pos = self.pressed, None, None
        if not card or event.button() != Qt.LeftButton or self.dragging:
            return
        if self.close_rect(card).contains(event.position()):
            self.dismiss(card)
        elif self.card_at(event.position()) is card:
            card.pinned = not card.pinned
            card.pin.target = 1. if card.pinned else 0.
            card.remaining = max(card.remaining, LIFETIME * .6)
            card.pin.velocity = 6.
        self.setCursor(Qt.OpenHandCursor if self.card_at(event.position()) else Qt.ArrowCursor)
        self._start()

    def grab(self, item):
        """Continue a drag the user began in the browser as a native file drag."""
        files = [f for f in item.get("files") or [] if isinstance(f, str) and Path(f).is_file()]
        if not files or sys.platform != "win32":
            return
        user32 = ctypes.windll.user32
        button = 0x02 if user32.GetSystemMetrics(23) else 0x01   # SM_SWAPBUTTON → physical primary button
        if not user32.GetAsyncKeyState(button) & 0x8000:
            log.info("grab ignored: the mouse button was already released")
            return
        card = Card(dict(item, files=files), False)
        self.grabber = self.grabber or Grabber(self)
        self.grabber.arm(card)

    def _drag(self, card, source=None):
        self.dragging = True
        self.pressed = None
        mime = QMimeData()
        mime.setUrls([QUrl.fromLocalFile(f) for f in card.files])
        # Some DAWs prefer a plain path list over CF_HDROP alone.
        mime.setData("text/plain", QByteArray("\r\n".join(card.files).encode("utf-8")))
        drag = QDrag(source or self)
        drag.setMimeData(mime)
        pixmap = self._ghost(card)
        drag.setPixmap(pixmap)
        drag.setHotSpot((QPointF(28, pixmap.height() / pixmap.devicePixelRatio() / 2)).toPoint())
        self.update()
        result = drag.exec(Qt.CopyAction | Qt.LinkAction, Qt.CopyAction)
        self.dragging = False
        if result != Qt.IgnoreAction:
            card.dropped_at = time.monotonic()
            card.flash.target = 1.
        if source is not None:
            return result
        self.hovered = self.card_at(self.mapFromGlobal(QCursor.pos()).toPointF())
        self._start()

    def _ghost(self, card):
        ratio = self.devicePixelRatioF()
        width, height = 260., 44.
        pixmap = QPixmap(int((width + 16) * ratio), int((height + 16) * ratio))
        pixmap.setDevicePixelRatio(ratio)
        pixmap.fill(Qt.transparent)
        p = QPainter(pixmap)
        p.setRenderHint(QPainter.Antialiasing)
        rect = QRectF(8, 8, width, height)
        self._shadow(p, rect, 14., 6)
        path = QPainterPath()
        path.addRoundedRect(rect, 14, 14)
        p.fillPath(path, RAISED)
        p.setPen(QPen(QColor(255, 255, 255, 40), 1))
        p.drawPath(path)
        self._glyph(p, QRectF(rect.left() + 12, rect.top() + 10, 24, 24), "audio", card.accent)
        p.setFont(self.title_font)
        p.setPen(TEXT)
        label = card.title if len(card.files) == 1 else f"{len(card.files)} files"
        text = QFontMetricsF(self.title_font).elidedText(label, Qt.ElideRight, width - 60)
        p.drawText(QRectF(rect.left() + 44, rect.top(), width - 56, height), Qt.AlignVCenter, text)
        p.end()
        return pixmap

    # Painting -----------------------------------------------------------
    def _shadow(self, p, rect, radius, layers=10):
        p.setPen(Qt.NoPen)
        for i in range(layers, 0, -1):
            grow = i * 2.2
            alpha = int(30 * (1 - i / (layers + 1)) ** 2)
            p.setBrush(QColor(0, 0, 0, alpha))
            p.drawRoundedRect(rect.adjusted(-grow, -grow + 4, grow, grow + 6), radius + grow, radius + grow)

    def _glyph(self, p, rect, glyph, color):
        """Draw a stroked icon (close, pin or audio) centred in rect, in one 1.5px weight."""
        size = min(rect.width(), rect.height()) * .46
        c = rect.center()
        path = QPainterPath()
        if glyph == "close":
            h = size / 2
            path.moveTo(c.x() - h, c.y() - h); path.lineTo(c.x() + h, c.y() + h)
            path.moveTo(c.x() + h, c.y() - h); path.lineTo(c.x() - h, c.y() + h)
        elif glyph == "pin":
            s = size / 10
            path.moveTo(c.x() - 3 * s, c.y() - 5 * s); path.lineTo(c.x() + 3 * s, c.y() - 5 * s)
            path.moveTo(c.x() - 2 * s, c.y() - 5 * s); path.lineTo(c.x() - 2.4 * s, c.y() + 0.5 * s)
            path.moveTo(c.x() + 2 * s, c.y() - 5 * s); path.lineTo(c.x() + 2.4 * s, c.y() + 0.5 * s)
            path.moveTo(c.x() - 4.5 * s, c.y() + 0.5 * s); path.lineTo(c.x() + 4.5 * s, c.y() + 0.5 * s)
            path.moveTo(c.x(), c.y() + 0.5 * s); path.lineTo(c.x(), c.y() + 6 * s)
        else:  # audio: the Siphon mark
            s = size / 8
            for outer, inner in ((4, 3), (1.5, 1.2)):
                path.moveTo(c.x() - outer * s, c.y() - 4 * s)
                path.lineTo(c.x() - outer * s, c.y() + 0 * s)
                path.arcTo(QRectF(c.x() - outer * s, c.y() - outer * s, 2 * outer * s, 2 * outer * s), 180, 180)
                path.lineTo(c.x() + outer * s, c.y() - 4 * s)
        pen = QPen(color, 1.5)
        pen.setCapStyle(Qt.RoundCap)
        pen.setJoinStyle(Qt.RoundJoin)
        p.save()
        p.setPen(pen)
        p.setBrush(Qt.NoBrush)
        p.drawPath(path)
        p.restore()

    def paintEvent(self, event):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        p.setRenderHint(QPainter.TextAntialiasing)
        for card in self.cards:
            self._paint_card(p, card)
        p.end()

    def _paint_card(self, p, card):
        presence = max(0., min(1., card.presence.value))
        if presence <= 0:
            return
        rect = self.card_rect(card)
        hover = card.hover.value
        p.save()
        p.setOpacity(min(1., presence * 1.25))
        self._shadow(p, rect, RADIUS)
        path = QPainterPath()
        path.addRoundedRect(rect, RADIUS, RADIUS)
        fill = QLinearGradient(rect.topLeft(), rect.bottomLeft())
        top = QColor(SURFACE).lighter(int(100 + 22 * hover))
        fill.setColorAt(0, top)
        fill.setColorAt(1, SURFACE)
        p.fillPath(path, fill)
        flash = card.flash.value
        if flash > .01:
            glow = QColor(card.accent)
            glow.setAlphaF(.14 * min(1., flash))
            p.fillPath(path, glow)
        # Specular top edge: the glass catches light along its upper rim.
        p.save()
        p.setClipRect(QRectF(rect.left(), rect.top(), rect.width(), RADIUS))
        p.setPen(QPen(SPECULAR, 1))
        p.drawPath(path.translated(0, .5))
        p.restore()
        # Pinned: a brighter neutral rim; only the pin itself carries the service colour.
        border = QColor(card.accent) if flash > .5 else QColor(255, 255, 255)
        border.setAlpha(int(120 * flash) if flash > .5 else int(22 + 40 * hover + 46 * card.pin.value))
        p.setPen(QPen(border, 1))
        p.drawPath(path)
        p.setClipPath(path)

        # Waveform tile: the cut start is the very first bar.
        tile = QRectF(rect.left() + 12, rect.top() + 11, 62, rect.height() - 22)
        tile_path = QPainterPath()
        tile_path.addRoundedRect(tile, 10, 10)
        p.fillPath(tile_path, QColor(255, 255, 255, int(12 + 8 * hover)))
        self._waveform(p, tile.adjusted(7, 6, -7, -6), card)

        # Title and subtitle; on hover the subtitle becomes the gesture hint.
        left = tile.right() + 12
        right = rect.right() - 44
        title_metrics = QFontMetricsF(self.title_font)
        p.setFont(self.title_font)
        p.setPen(TEXT)
        p.drawText(QRectF(left, rect.top() + 13, right - left, 20), Qt.AlignLeft | Qt.AlignVCenter,
                   title_metrics.elidedText(card.title, Qt.ElideRight, right - left))
        p.setFont(self.sub_font)
        sub_metrics = QFontMetricsF(self.sub_font)
        if card.dropped_at:
            hint = "Dropped" if len(card.files) == 1 else f"Dropped {len(card.files)} files"
        elif card.pinned:
            hint = "Pinned · drag into your DAW · click to release"
        else:
            hint = "Drag into your DAW · click to pin"
        sub_rect = QRectF(left, rect.top() + 34, right - left, 18)
        for text, alpha, offset in ((card.subtitle, 1 - hover, -4 * hover), (hint, hover if not card.dropped_at else 1, 4 * (1 - hover))):
            if card.dropped_at and text is card.subtitle:
                continue
            color = QColor(card.accent if card.dropped_at or (text is hint and card.pinned) else MUTED)
            color.setAlphaF(max(0., min(1., alpha)))
            p.setPen(color)
            p.drawText(sub_rect.translated(0, offset), Qt.AlignLeft | Qt.AlignVCenter, sub_metrics.elidedText(text, Qt.ElideRight, right - left))

        # Close appears on hover; the pin badge stays while pinned.
        close = self.close_rect(card)
        cursor = self.mapFromGlobal(QCursor.pos()).toPointF()
        if hover > .01:
            hot = close.contains(cursor)
            p.setPen(Qt.NoPen)
            p.setBrush(QColor(255, 255, 255, int((30 if hot else 12) * hover)))
            p.drawEllipse(close)
            color = QColor(TEXT if hot else MUTED)
            color.setAlphaF(hover)
            self._glyph(p, close, "close", color)
        pin = card.pin.value
        if pin > .01:
            badge = QRectF(close.left() + 3, close.bottom() + 6, 18, 18)
            c = badge.center()
            badge.setSize(badge.size() * max(0., pin))
            badge.moveCenter(c)
            color = QColor(card.accent)
            color.setAlphaF(min(1., pin))
            self._glyph(p, badge, "pin", color)

        # Countdown: a hairline that drains right-to-left and dims while paused.
        if not card.pinned and not card.dropped_at:
            fraction = max(0., min(1., card.remaining / LIFETIME))
            track = QRectF(rect.left() + RADIUS, rect.bottom() - 3.5, rect.width() - 2 * RADIUS, 2)
            p.setPen(Qt.NoPen)
            p.setBrush(QColor(255, 255, 255, 14))
            p.drawRoundedRect(track, 1, 1)
            bar = QColor(card.accent)
            bar.setAlphaF(.45 if self.hovered is not None or self.dragging else .9)
            p.setBrush(bar)
            p.drawRoundedRect(QRectF(track.left(), track.top(), track.width() * fraction, track.height()), 1, 1)
        p.restore()

    def _waveform(self, p, area, card):
        values = card.peaks
        count = len(values) if values else 18
        step = area.width() / count
        width = max(1.2, step * .55)
        grow = card.reveal.value if values else 0.
        middle = area.center().y()
        breathe = time.monotonic() * 3.2
        p.setPen(Qt.NoPen)
        for i in range(count):
            if values:
                level = .08 + .92 * values[i] * max(0., min(1.2, grow * 1.4 - i / count * .4))
            else:
                # Measuring: a quiet idle shimmer instead of a spinner.
                level = .16 + .1 * (1 + math.sin(breathe - i * .55)) / 2
            height = max(2., level * area.height())
            # Neutral waveform; its first bar (the trimmed start) carries the service colour.
            color = QColor(card.accent if i == 0 and values else TEXT)
            color.setAlphaF(1. if i == 0 and values else .62)
            p.setBrush(color)
            x = area.left() + i * step + (step - width) / 2
            p.drawRoundedRect(QRectF(x, middle - height / 2, width, height), width / 2, width / 2)
        if not values:
            self._start()


class Grabber(QWidget):
    """A card-sized, focusless window that receives the re-pressed button.

    The browser owns the mouse capture while its button is held, and Windows
    won't let another process start a drag from it. Placing this window under
    the cursor and replaying the press (up, then down) moves the press here,
    where a normal QDrag begins. The physical release then drops the file.
    """

    def __init__(self, tray):
        super().__init__(None, Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Tool | Qt.WindowDoesNotAcceptFocus
                         | Qt.NoDropShadowWindowHint)
        self.setAttribute(Qt.WA_TranslucentBackground)
        self.setAttribute(Qt.WA_ShowWithoutActivating)
        self.tray, self.card, self.pixmap = tray, None, None
        self.expire = QTimer(self, singleShot=True, interval=700)
        self.expire.timeout.connect(self.hide)

    def arm(self, card):
        self.card = card
        self.pixmap = self.tray._ghost(card)
        self.resize(self.pixmap.size() / self.pixmap.devicePixelRatio())
        self._follow()
        self.show()
        self.raise_()
        self.attempts = 0
        self.expire.stop()
        QTimer.singleShot(0, self._replay)

    def _follow(self):
        cursor = QCursor.pos()
        self.move(cursor.x() - 28, cursor.y() - self.height() // 2)

    def _replay(self):
        """Move the held press onto this window, only once it is really under the cursor.

        Replaying earlier would deliver the press to whatever lies beneath,
        which is an unwanted click in another app. So: wait for the window
        to be hit-testable at the cursor, and abort if it never is.
        """
        if self.card is None:
            return
        user32 = ctypes.windll.user32
        swapped = user32.GetSystemMetrics(23)
        if not user32.GetAsyncKeyState(0x02 if swapped else 0x01) & 0x8000:
            log.info("grab abandoned: the mouse button was released first")
            return self._abort()
        self._follow()
        point = QCursor.pos()
        from ctypes import wintypes
        under = user32.GetAncestor(user32.WindowFromPoint(wintypes.POINT(point.x(), point.y())), 2)  # GA_ROOT
        if under != int(self.winId()):
            self.attempts += 1
            if self.attempts > 25:
                log.warning("grab abandoned: the drag card never appeared under the cursor")
                return self._abort()
            return QTimer.singleShot(10, self._replay)
        up, down = (0x10, 0x08) if swapped else (0x04, 0x02)  # MOUSEEVENTF_{RIGHT,LEFT}{UP,DOWN}
        user32.mouse_event(up, 0, 0, 0, 0)
        user32.mouse_event(down, 0, 0, 0, 0)
        log.debug("grab replayed after %d checks at %s", self.attempts, point)
        self.expire.start()

    def _abort(self):
        self.card = None
        self.hide()

    def paintEvent(self, event):
        p = QPainter(self)
        # An almost invisible fill keeps the whole window hit-testable;
        # fully transparent pixels would let the replayed press fall through.
        p.fillRect(self.rect(), QColor(0, 0, 0, 1))
        if self.pixmap:
            p.drawPixmap(0, 0, self.pixmap)
        p.end()

    def mousePressEvent(self, event):
        log.debug("grab press received")
        self.expire.stop()
        card, self.card = self.card, None
        if card is None:
            return self.hide()
        result = self.tray._drag(card, source=self)
        log.debug("grab drag finished: %s", result)
        self.hide()
        if result != Qt.IgnoreAction:
            # Confirm the drop with a brief card in the tray.
            card.pinned, card.remaining = False, 1.2
            self.tray.accept({"items": [{"files": card.files, "title": card.title, "subtitle": card.subtitle}]})
            for c in self.tray.cards:
                if c.files == card.files:
                    c.dropped_at, c.flash.target = time.monotonic(), 1.

    def mouseReleaseEvent(self, event):
        self.hide()


def host_alive(pid):
    if not pid or sys.platform != "win32":
        return True
    kernel = ctypes.windll.kernel32
    handle = kernel.OpenProcess(0x00100000, False, int(pid))  # SYNCHRONIZE
    if not handle:
        return False
    try:
        return kernel.WaitForSingleObject(handle, 0) == 0x102  # WAIT_TIMEOUT: still running
    finally:
        kernel.CloseHandle(handle)


def dispatch(tray, payload):
    if payload.get("grab"):
        tray.grab(payload["grab"])
    else:
        tray.accept(payload)


def read_payload(stream):
    data = stream.read() if stream else ""
    payload = json.loads(data or "{}")
    if not isinstance(payload, dict):
        raise ValueError("payload must be an object")
    return payload


def configure_logging():
    from .runtime import local_app_data
    folder = local_app_data() / "Siphon" / "logs"
    try:
        folder.mkdir(parents=True, exist_ok=True)
        handler = logging.FileHandler(folder / "tray.log", encoding="utf-8")
    except OSError:
        return
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(message)s"))
    log.addHandler(handler)
    log.setLevel(logging.DEBUG if os.environ.get("SIPHON_TRAY_DEBUG") else logging.INFO)


def main(argv=None):
    configure_logging()
    try:
        payload = read_payload(sys.stdin)
    except (OSError, ValueError, AttributeError):
        return 2
    QGuiApplication.setHighDpiScaleFactorRoundingPolicy(Qt.HighDpiScaleFactorRoundingPolicy.PassThrough)
    app = QApplication(sys.argv[:1])
    load_font()
    app.setQuitOnLastWindowClosed(False)
    message = json.dumps(payload).encode("utf-8") + b"\n"
    socket = QLocalSocket()
    socket.connectToServer(SERVER)
    if socket.waitForConnected(400):
        socket.write(message)
        socket.waitForBytesWritten(1000)
        socket.disconnectFromServer()
        return 0
    QLocalServer.removeServer(SERVER)
    server = QLocalServer()
    server.setSocketOptions(QLocalServer.UserAccessOption)
    server.listen(SERVER)
    tray = Tray(payload.get("ffmpeg"))

    def connection():
        client = server.nextPendingConnection()
        buffer = bytearray()

        def readable():
            buffer.extend(bytes(client.readAll()))
            while b"\n" in buffer:
                line, _, rest = bytes(buffer).partition(b"\n")
                buffer[:] = rest
                try:
                    dispatch(tray, json.loads(line))
                except (ValueError, TypeError, AttributeError):
                    log.exception("Rejected tray message")
        client.readyRead.connect(readable)
        client.disconnected.connect(client.deleteLater)
    server.newConnection.connect(connection)
    if payload.get("resident"):
        tray.resident = True
        host = payload.get("hostPid")
        watchdog = QTimer(interval=5000)
        watchdog.timeout.connect(lambda: tray.cards or host_alive(host) or app.quit())
        watchdog.start()
    dispatch(tray, payload)
    if not tray.cards and not tray.resident:
        return 0
    return app.exec()


if __name__ == "__main__":
    sys.exit(main())
