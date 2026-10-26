"""Late BPM/key refresh a visible tray card in place (headless Qt)."""

import os

import pytest

pytest.importorskip("PySide6")
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication  # noqa: E402

from siphon_host import dropzone  # noqa: E402


@pytest.fixture
def tray(monkeypatch):
    app = QApplication.instance() or QApplication([])
    monkeypatch.setattr(dropzone.Tray, "_measure", lambda self, card: None)
    widget = dropzone.Tray(None)
    yield widget
    widget.close()
    app.processEvents()


def test_replaces_updates_visible_card_and_never_resummons(tray, tmp_path):
    before, after = tmp_path / "a.mp3", tmp_path / "a [120 BPM Am].mp3"
    before.write_bytes(b"x")
    tray.accept({"items": [{"files": [str(before)], "title": "Song", "subtitle": "MP3"}]})
    card = tray.cards[-1]
    before.rename(after)
    tray.accept({"items": [{"files": [str(after)], "subtitle": "120 BPM · Am · MP3", "replaces": [str(before)]}]})
    assert len(tray.cards) == 1 and card.files == [str(after)]
    assert card.subtitle == "120 BPM · Am · MP3" and card.title == "Song"
    tray.dismiss(card)
    count = len([c for c in tray.cards if not c.closing])
    tray.accept({"items": [{"files": [str(after)], "subtitle": "later", "replaces": [str(after)]}]})
    assert len([c for c in tray.cards if not c.closing]) == count == 0
