/** User-facing recovery advice for every protocol error, shared by all extension UIs. */
export const ERROR_MESSAGES = Object.freeze({
  HOST_NOT_FOUND: 'Install the Siphon native host from Settings → Connection & setup.',
  UNSUPPORTED_URL: 'No supported audio found. Open a track, album, or video link and try again.',
  NETWORK: 'The source could not be reached. Check your connection and try again.',
  GEO_BLOCKED: 'This audio is unavailable in your region.',
  LOGIN_REQUIRED: 'Sign in on the site and enable “Send site cookies” in Settings → Sites & playlists.',
  AGE_RESTRICTED: 'Sign in with an eligible account and enable “Send site cookies” in Settings → Sites & playlists.',
  DRM: 'This audio is protected by DRM and cannot be downloaded.',
  LIVE_NOT_SUPPORTED: 'Wait for this live stream to finish before downloading.',
  FFMPEG_MISSING: 'FFmpeg is missing. Open Settings → Connection & setup to install the audio converter.',
  CANCELLED: 'Download cancelled. You can retry whenever you’re ready.',
  DISK: 'The file could not be saved or opened. Check free space, folder permissions, and whether the file still exists.',
  BAD_REQUEST: 'Check the download settings and try again.',
  INTERNAL: 'The audio engine ran into a problem. Reconnect in Settings → Connection & setup and try again.',
});

export function friendlyError(error) {
  // Validation errors already identify the field/action the user can correct.
  if (error?.code === 'BAD_REQUEST' && error.message) return error.message;
  return ERROR_MESSAGES[error?.code] || error?.message || (typeof error === 'string' && error) || 'Something interrupted Siphon. Please try again.';
}
