import { StrictMode } from 'react'
import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AnalysisResult, GameUpdate } from '@/types'

const mocks = vi.hoisted(() => {
  Object.defineProperty(globalThis, '__TAURI__', { value: {}, configurable: true })
  return { invoke: vi.fn(), listen: vi.fn() }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }))
vi.mock('@/components/ui/sonner', () => ({ toast: { info: vi.fn(), success: vi.fn(), warning: vi.fn(), error: vi.fn() } }))

import { createEventScope } from '@/lib/tauri'
import { useTauriBridge } from './useTauriBridge'
import { useGameStore } from '@/stores/gameStore'
import { useAnalysisStore } from '@/stores/analysisStore'
import { useBotStore } from '@/stores/botStore'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((r) => { resolve = r })
  return { promise, resolve }
}
function update(revision: number): GameUpdate {
  return {
    revision,
    game: { bakaze: 'E', kyoku: 1, honba: 0, kyotaku: 0, oya: 0, current_player: 0,
      turn_count: revision, phase: 'wait_act', is_done: false, num_players: 4,
      players: [], dora_markers: [], our_seat: 0 },
    view: { players: [], num_players: 4, dora_indicators: `${revision}m` },
  }
}
function analysis(revision: number): AnalysisResult {
  return { revision, seat: 0, turn: 1, shanten: 0, state: 'wait13', hand13: null,
    hand14: null, opponents: [], mixed_risk: [], best_attack_discard: null, best_defence_discard: null }
}

beforeEach(() => {
  vi.clearAllMocks()
  useGameStore.setState({ game: null, view: null, revision: -1 })
  useAnalysisStore.getState().set(null)
  mocks.invoke.mockImplementation((cmd: string) => {
    if (cmd === 'get_status') return new Promise(() => {})
    if (cmd === 'get_game_update') return Promise.resolve(update(0))
    if (cmd === 'list_game_history') return Promise.resolve([])
    return Promise.resolve(null)
  })
  mocks.listen.mockResolvedValue(vi.fn())
})

describe('Tauri bridge lifetime and ordering', () => {
  it('unsubscribes registrations that resolve after StrictMode cleanup and unmount', async () => {
    const registrations: ReturnType<typeof deferred<() => void>>[] = []
    mocks.listen.mockImplementation(() => {
      const registration = deferred<() => void>()
      registrations.push(registration)
      return registration.promise
    })
    const { unmount } = renderHook(() => useTauriBridge(), { wrapper: StrictMode })
    expect(registrations).toHaveLength(18)
    unmount()
    const unlisten = vi.fn()
    await act(async () => { registrations.forEach((registration) => registration.resolve(unlisten)) })
    expect(unlisten).toHaveBeenCalledTimes(18)
    expect(mocks.invoke).not.toHaveBeenCalled()
  })

  it('stops callbacks immediately and unregisters an existing scope only once', async () => {
    const callback = vi.fn()
    const unlisten = vi.fn()
    let deliver!: (event: { payload: string }) => void
    mocks.listen.mockImplementation((_name: string, cb: typeof deliver) => {
      deliver = cb
      return Promise.resolve(unlisten)
    })
    const scope = createEventScope()
    await scope.listen<string>('example', callback)
    deliver({ payload: 'before' })
    scope.dispose()
    deliver({ payload: 'after' })
    scope.dispose()
    expect(callback).toHaveBeenCalledExactlyOnceWith('before')
    expect(unlisten).toHaveBeenCalledTimes(1)
  })

  it('keeps a live game and bot status when delayed hydration replies arrive', async () => {
    const status = deferred<unknown>()
    const initialGame = deferred<GameUpdate>()
    const callbacks = new Map<string, (event: { payload: unknown }) => void>()
    mocks.listen.mockImplementation((name: string, cb: (event: { payload: unknown }) => void) => {
      callbacks.set(name, cb)
      return Promise.resolve(vi.fn())
    })
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_status') return status.promise
      if (cmd === 'get_game_update') return initialGame.promise
      if (cmd === 'list_game_history') return Promise.resolve([])
      return Promise.resolve(null)
    })
    const { unmount } = renderHook(() => useTauriBridge())
    await act(async () => {})
    act(() => {
      callbacks.get('game-update')!({ payload: update(2) })
      callbacks.get('bot-status')!({ payload: { state: 'stopped' } })
      callbacks.get('analysis-result')!({ payload: analysis(2) })
    })
    await act(async () => status.resolve({ config: {}, log_dir: '', bot_status: { state: 'idle' }, capture_status: { state: 'stopped' } }))
    await act(async () => initialGame.resolve(update(1)))
    expect(useGameStore.getState().revision).toBe(2)
    expect(useGameStore.getState().view?.dora_indicators).toBe('2m')
    expect(useAnalysisStore.getState().result?.revision).toBe(2)
    expect(useBotStore.getState().status).toEqual({ state: 'stopped' })
    act(() => callbacks.get('game-update')!({ payload: { revision: 3, game: null, view: null } }))
    act(() => callbacks.get('analysis-result')!({ payload: analysis(2) }))
    expect(useGameStore.getState().game).toBeNull()
    expect(useAnalysisStore.getState().result).toBeNull()
    act(() => callbacks.get('analysis-result')!({ payload: analysis(4) }))
    expect(useAnalysisStore.getState().result).toBeNull()
    act(() => callbacks.get('game-update')!({ payload: update(4) }))
    expect(useAnalysisStore.getState().result?.revision).toBe(4)
    act(() => callbacks.get('analysis-result')!({ payload: analysis(5) }))
    act(() => callbacks.get('game-update')!({ payload: update(6) }))
    expect(useAnalysisStore.getState().result).toBeNull()
    unmount()
  })

  it('rejects old game and analysis versions even after a bridge remount', () => {
    useGameStore.getState().setUpdate(update(2))
    useGameStore.getState().setUpdate(update(1))
    expect(useGameStore.getState().revision).toBe(2)
    useAnalysisStore.getState().set(analysis(2))
    useAnalysisStore.getState().set(analysis(1))
    expect(useAnalysisStore.getState().result?.revision).toBe(2)
  })
})
