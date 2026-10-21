import pytest


@pytest.fixture(autouse=True)
def no_drag_tray(monkeypatch):
    """Never start the real drag tray (a GUI process) from unit tests."""
    from siphon_host.host import Host
    monkeypatch.setattr(Host, "_warm_tray", lambda self: None)
