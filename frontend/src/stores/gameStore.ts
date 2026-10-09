import { create } from 'zustand'
import type { GameStateSnapshot, GameUpdate, MahgenView } from '@/types'

type GameStore = {
  game: GameStateSnapshot | null
  view: MahgenView | null
  revision: number
  setUpdate: (update: GameUpdate) => void
}

export const useGameStore = create<GameStore>((set) => ({
  game: null,
  view: null,
  revision: -1,
  setUpdate: (update) => set((current) => update.revision < current.revision ? current : update),
}))

/** Active player count for the current game. Defaults to 4 when no game is in
 * progress. UI components should use this for any seat-relative arithmetic
 * (winds, kamicha/shimocha, etc.) so 3p (sanma) and 4p stay correct. */
export const useNumPlayers = () => useGameStore((s) => s.game?.num_players ?? 4)
