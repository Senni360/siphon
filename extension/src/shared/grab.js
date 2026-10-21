import { CMD, send } from './protocol.js';

/**
 * Drag a downloaded file out of an extension page into another app.
 * A browser page can only drag a temporary copy, so once the pointer moves
 * past a small threshold, the native tray takes over the held button and
 * continues the drag with the real file (see host dropzone.Grabber).
 * Rows inside `container` that match `selector` carry `data-grab-id`.
 */
export function enableGrab(container, selector = '[data-grab-id]') {
  container.addEventListener('dragstart', event => { if (event.target.closest?.(selector)) event.preventDefault(); });
  container.addEventListener('pointerdown', event => {
    const row = event.target.closest(selector);
    if (!row || event.button !== 0 || !event.isPrimary || event.target.closest('a, input, select, textarea')) return;
    const { clientX: x, clientY: y } = event;
    let started = false;
    const move = moved => {
      if (started || Math.hypot(moved.clientX - x, moved.clientY - y) < 5) return;
      started = true;
      row.classList.add('grabbing');
      void send(CMD.GRAB, { id: row.dataset.grabId }).catch(() => {});
      finish();
    };
    const finish = () => {
      removeEventListener('pointermove', move, true);
      removeEventListener('pointerup', finish, true);
      removeEventListener('pointercancel', finish, true);
      if (started) setTimeout(() => row.classList.remove('grabbing'), 400);
    };
    addEventListener('pointermove', move, true);
    addEventListener('pointerup', finish, true);
    addEventListener('pointercancel', finish, true);
  });
  // The tray replays the press, which ends in a click; it must not trigger row buttons.
  container.addEventListener('click', event => {
    const row = event.target.closest(selector);
    if (row?.classList.contains('grabbing')) { event.preventDefault(); event.stopPropagation(); }
  }, true);
}
