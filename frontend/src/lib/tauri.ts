import { invoke as tauriInvoke } from '@tauri-apps/api/core'
import { listen as tauriListen, type UnlistenFn } from '@tauri-apps/api/event'

// SAFETY: Tauri injects this optional global only inside its desktop webviews.
const TAURI_GLOBAL = (globalThis as unknown as { __TAURI__?: unknown }).__TAURI__
export const HAS_TAURI = TAURI_GLOBAL !== undefined

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!HAS_TAURI) throw new Error(`tauri not available: ${cmd}`)
  return await tauriInvoke<T>(cmd, args)
}

export async function listen<T>(
  name: string,
  cb: (payload: T) => void,
): Promise<UnlistenFn> {
  if (!HAS_TAURI) return () => {}
  return await tauriListen<T>(name, (e) => cb(e.payload))
}

/** Group async registrations under one component lifetime. Late registration
 * completion is immediately undone, and callbacks stop at disposal. */
export function createEventScope() {
  let disposed = false
  const unlistens: UnlistenFn[] = []
  return {
    async listen<T>(name: string, cb: (payload: T) => void) {
      const unlisten = await listen<T>(name, (payload) => {
        if (!disposed) cb(payload)
      })
      if (disposed) unlisten()
      else unlistens.push(unlisten)
    },
    dispose() {
      disposed = true
      unlistens.splice(0).forEach((unlisten) => unlisten())
    },
  }
}
