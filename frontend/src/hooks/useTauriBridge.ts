import { useEffect } from 'react'
import { HAS_TAURI, createEventScope, invoke } from '@/lib/tauri'
import type {
  AnalysisResult, BotResponse, BotStatus, CaptureStatus, GameRecord,
  GameUpdate, HistoryEvent, MjaiEvent, Notification, OverlayConfig, Snapshot,
} from '@/types'
import { useGameStore } from '@/stores/gameStore'
import { useAnalysisStore } from '@/stores/analysisStore'
import { useBotStore } from '@/stores/botStore'
import { useCaptureStore } from '@/stores/captureStore'
import { useNotifyStore } from '@/stores/notifyStore'
import { useApiStatusStore } from '@/stores/apiStatusStore'
import { useInstallStore } from '@/stores/installStore'
import { useConfigStore } from '@/stores/configStore'
import { useHistoryStore } from '@/stores/historyStore'
import { toast, type ToastSeverity } from '@/components/ui/sonner'

const TOAST_SEVERITY: Record<Notification['level'], ToastSeverity> = {
  info: 'info', success: 'success', warn: 'warning', error: 'error',
}

// Register streams before hydrating. Revisions prevent late IPC replies from
// overwriting a newer game/analysis event, including after window remounts.
export function useTauriBridge() {
  useEffect(() => {
    if (!HAS_TAURI) return
    const scope = createEventScope()
    let cancelled = false
    let liveBotStatus = false
    let liveCaptureStatus = false
    let liveOverlay = false
    let historyLoaded = false
    let pendingAnalysis: AnalysisResult | null = null
    const historyPending: HistoryEvent[] = []
    const applyHistory = (ev: HistoryEvent) => {
      if (ev.kind === 'recorded') useHistoryStore.getState().prepend(ev.record)
      else if (ev.kind === 'deleted') useHistoryStore.getState().remove(ev.id)
    }
    const applyGame = (update: GameUpdate) => {
      if (update.revision < useGameStore.getState().revision) return
      useGameStore.getState().setUpdate(update)
      if (!update.game || (useAnalysisStore.getState().result?.revision ?? -1) < update.revision) {
        useAnalysisStore.getState().set(null)
      }
      if (pendingAnalysis && pendingAnalysis.revision <= update.revision) {
        if (update.game && pendingAnalysis.revision === update.revision) {
          useAnalysisStore.getState().set(pendingAnalysis)
        }
        pendingAnalysis = null
      }
    }
    const applyAnalysis = (result: AnalysisResult | null) => {
      if (!result) return
      const { revision, game } = useGameStore.getState()
      if (game && result.revision === revision) useAnalysisStore.getState().set(result)
      else if (result.revision > revision && result.revision >= (pendingAnalysis?.revision ?? -1)) {
        pendingAnalysis = result
      }
    }
    const subscriptions = [
      scope.listen<MjaiEvent>('mjai-event', (e) => {
        if (e.type === 'start_game') useApiStatusStore.getState().reset()
        useNotifyStore.getState().pushEvent(e)
      }),
      scope.listen<GameUpdate>('game-update', applyGame),
      scope.listen<AnalysisResult>('analysis-result', applyAnalysis),
      scope.listen<BotStatus>('bot-status', (s) => {
        liveBotStatus = true
        useBotStore.getState().setStatus(s)
      }),
      scope.listen<CaptureStatus>('capture-status', (s) => {
        liveCaptureStatus = true
        useCaptureStore.getState().set(s)
      }),
      scope.listen<BotResponse>('bot-response', (r) => useNotifyStore.getState().pushResponse(r)),
      scope.listen<OverlayConfig>('overlay-config', (o) => {
        liveOverlay = true
        useConfigStore.getState().setOverlay(o)
      }),
      scope.listen<HistoryEvent>('history-recorded', (ev) => {
        if (historyLoaded) applyHistory(ev)
        else historyPending.push(ev)
      }),
      scope.listen<Notification>('notify', (n) => {
        useNotifyStore.getState().pushToast(n)
        if (n.id === 'native-api-health') {
          useApiStatusStore.getState().setDegraded(n.level === 'warn' || n.level === 'error', n.body)
        }
        if (n.id && (n.id.startsWith('bot-install-') || n.id.startsWith('bot-sync-'))) {
          useInstallStore.getState().setProgress(n)
        }
        toast[TOAST_SEVERITY[n.level]](n.title, {
          description: n.body, id: n.id,
          ...(n.sticky ? { duration: Infinity } : {}),
        })
      }),
    ]
    void (async () => {
      const registered = await Promise.allSettled(subscriptions)
      if (cancelled) return
      for (const result of registered) {
        if (result.status === 'rejected') console.error('Tauri event registration failed', result.reason)
      }
      try {
        const status = await invoke<Snapshot>('get_status')
        if (cancelled) return
        const overlay = useConfigStore.getState().config?.overlay
        useConfigStore.getState().setConfig(status.config)
        if (liveOverlay && overlay) useConfigStore.getState().setOverlay(overlay)
        useConfigStore.getState().setLogDir(status.log_dir)
        if (!liveBotStatus) useBotStore.getState().setStatus(status.bot_status)
        if (!liveCaptureStatus) useCaptureStore.getState().set(status.capture_status)
      } catch (error) {
        console.error('Tauri status hydration failed', error)
      }
      if (cancelled) return
      try {
        const update = await invoke<GameUpdate>('get_game_update')
        if (!cancelled) applyGame(update)
      } catch (error) {
        console.error('Tauri game hydration failed', error)
      }
      if (cancelled) return
      try {
        const analysis = await invoke<AnalysisResult | null>('get_analysis')
        if (!cancelled) applyAnalysis(analysis)
      } catch (error) {
        console.error('Tauri analysis hydration failed', error)
      }
      if (cancelled) return
      try {
        useHistoryStore.getState().setLoading(true)
        const records = await invoke<GameRecord[]>('list_game_history', {
          filter: null, limit: 1_000_000, offset: 0,
        })
        if (!cancelled) useHistoryStore.getState().setRecords(records)
      } catch (error) {
        console.error('Tauri history hydration failed', error)
      } finally {
        if (!cancelled) {
          historyLoaded = true
          historyPending.forEach(applyHistory)
          historyPending.length = 0
          useHistoryStore.getState().setLoading(false)
        }
      }
    })()
    return () => {
      cancelled = true
      scope.dispose()
    }
  }, [])
}
