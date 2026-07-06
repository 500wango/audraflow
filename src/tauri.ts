import { invoke, isTauri as detectTauriRuntime } from '@tauri-apps/api/core';

type TauriRuntimeWindow = Window & {
  __TAURI_INTERNALS__?: { invoke?: unknown };
  isTauri?: boolean;
};

export function hasTauriRuntime(): boolean {
  if (typeof window === 'undefined') return false;
  const tauriWindow = window as TauriRuntimeWindow;
  return (
    detectTauriRuntime() ||
    tauriWindow.isTauri === true ||
    typeof tauriWindow.__TAURI_INTERNALS__?.invoke === 'function'
  );
}

export type Translate = (key: string, params?: Record<string, string | number>) => string;

export function localTranscriptionUnavailableText(t: Translate): string {
  return hasTauriRuntime() ? t('model.required') : t('model.desktopRequired');
}

export async function invokeTauri<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    if (!hasTauriRuntime() && isMissingTauriRuntimeError(error)) {
      throw new Error('This action requires the Tauri desktop app window, not the browser page.', {
        cause: error,
      });
    }
    throw error;
  }
}

export function rawErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function errorToMessage(error: unknown, t?: Translate): string {
  const message = rawErrorMessage(error);
  return t ? friendlyErrorMessage(message, t) : message;
}

function friendlyErrorMessage(message: string, t: Translate): string {
  const normalized = message.toLowerCase();
  const withDetail = (key: string) => t(key, { detail: message });
  const missingExecutable =
    normalized.includes('was not found') ||
    normalized.includes('is required') ||
    normalized.includes('no such file') ||
    normalized.includes('os error 2');

  if (normalized.includes('command plugin:dialog|save not allowed by acl') ||
      normalized.includes('plugin:dialog|save') ||
      (normalized.includes('not allowed by acl') && normalized.includes('dialog'))) {
    return withDetail('error.dialogAcl');
  }
  if (normalized.includes('this action requires the tauri desktop app') ||
      normalized.includes('tauri desktop app window') ||
      isMissingTauriRuntimeMessage(normalized)) {
    return withDetail('error.desktopRequired');
  }
  if (normalized.includes('no space left') ||
      normalized.includes('disk full') ||
      normalized.includes('not enough space') ||
      normalized.includes('磁盘空间')) {
    return withDetail('error.diskSpace');
  }
  if (normalized.includes('permission denied') ||
      normalized.includes('access is denied') ||
      normalized.includes('eacces') ||
      normalized.includes('权限不够')) {
    return withDetail('error.permissionDenied');
  }
  if (normalized.includes('yt-dlp is required') ||
      normalized.includes('yt-dlp was not found') ||
      (normalized.includes('failed to run yt-dlp') && missingExecutable) ||
      normalized.includes('audraflow_yt_dlp_bin')) {
    return withDetail('error.ytDlpMissing');
  }
  if (normalized.includes('url returned an html page')) {
    return withDetail('error.directUrlHtml');
  }
  if (normalized.includes('yt-dlp could not download') ||
      normalized.includes('http error 403') ||
      normalized.includes('403: forbidden') ||
      normalized.includes('content is not available') ||
      normalized.includes('latest version of youtube') ||
      normalized.includes('platform download failed')) {
    return withDetail('error.platformDownloadFailed');
  }
  if (normalized.includes('timed out') || normalized.includes('timeout')) {
    return withDetail('error.timeout');
  }
  if (normalized.includes('ffprobe') &&
      (missingExecutable || normalized.includes('audraflow_ffprobe_bin'))) {
    return withDetail('error.ffprobeMissing');
  }
  if (normalized.includes('ffmpeg') &&
      (missingExecutable || normalized.includes('audraflow_ffmpeg_bin'))) {
    return withDetail('error.ffmpegMissing');
  }
  if (normalized.includes('selected asr model file is missing')) {
    return withDetail('error.modelFileMissing');
  }
  if (normalized.includes('no asr model is selected')) {
    return withDetail('error.noModelSelected');
  }
  if (normalized.includes('vcruntime140') ||
      normalized.includes('msvcp140') ||
      normalized.includes('0xc0000135') ||
      normalized.includes('visual c++ runtime') ||
      normalized.includes('vc++ runtime')) {
    return withDetail('error.vcRedistMissing');
  }
  if (normalized.includes('whisper cli') || normalized.includes('whisper-cli')) {
    return withDetail('error.whisperCliMissing');
  }
  if (normalized.includes('sensevoice requires python')) {
    return withDetail('error.sensevoicePythonMissing');
  }
  if (normalized.includes('funasr and modelscope') ||
      normalized.includes('sensevoice requires python packages')) {
    return withDetail('error.sensevoicePackagesMissing');
  }
  if (normalized.includes('fun-asr') || normalized.includes('funasr')) {
    if (normalized.includes('gguf') || normalized.includes('model')) {
      return withDetail('error.funasrModelsMissing');
    }
    return withDetail('error.funasrCliMissing');
  }
  if (normalized.includes('failed to download') ||
      normalized.includes('download failed') ||
      normalized.includes('connection refused') ||
      normalized.includes('network')) {
    return withDetail('error.networkDownload');
  }

  return message;
}

function isMissingTauriRuntimeMessage(message: string): boolean {
  return (
    message.includes('__tauri') ||
    message.includes('tauri internals') ||
    message.includes('not in a tauri') ||
    message.includes('invoke is not a function') ||
    message.includes('cannot read properties of undefined')
  );
}

function isMissingTauriRuntimeError(error: unknown): boolean {
  const message = errorToMessage(error).toLowerCase();
  return isMissingTauriRuntimeMessage(message);
}
