// ZOUGCLOUD(ZC-008/ZC-009): per-game state the fork adds to the game page.
//
// Kept in its own composable so the patch to upstream's game page stays a
// handful of lines, and so a future maintainer can delete this file outright if
// upstream ever grows equivalent features.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type PlaytimeSummary = {
  totalSeconds: number;
  lastPlayedAt: number | null;
  /** Pre-formatted in Rust; null when the game has never been played. */
  display: string | null;
  lastPlayedDisplay: string | null;
  /** A session is open right now, so totalSeconds is not yet final. */
  active: boolean;
};

type SteamGameStatus = {
  install: unknown | null;
  shortcut: { appId: number } | null;
};

/**
 * Playtime and Steam-shortcut state for one game.
 *
 * The Steam side is deliberately read from Steam's own shortcuts file on every
 * refresh rather than from a cached flag: if someone deletes the shortcut from
 * inside Steam, the button has to disappear, and a local boolean would keep
 * claiming it exists.
 */
export const useZougcloudGameState = async (gameId: string, appName: string) => {
  const playtime = ref<PlaytimeSummary | null>(null);
  const steamAppId = ref<number | null>(null);

  async function refresh() {
    playtime.value = await invoke<PlaytimeSummary>("fetch_playtime", { gameId });

    try {
      const steam = await invoke<SteamGameStatus>("steam_game_status", {
        gameId,
        appName,
        accountId: null,
      });
      steamAppId.value =
        steam.install && steam.shortcut ? steam.shortcut.appId : null;
    } catch {
      // Steam missing, or an account we cannot read: simply no button.
      steamAppId.value = null;
    }
  }

  await refresh();

  // A session the watcher closed (game launched from Steam) has no other way to
  // reach an already-open page.
  listen("zougcloud:playtime-updated", (event) => {
    if (event.payload === gameId) refresh();
  });

  // Covers the Drop-launched case: upstream already emits this when the process
  // exits, which is exactly when the session closed.
  listen(`update_game/${gameId}`, () => refresh());

  /**
   * Open Steam on this game.
   *
   * Uses the shortcut's existing stable AppID. It never recreates the shortcut,
   * never touches its artwork and never changes its identity — Steam keys
   * playtime, controller settings and artwork filenames on that AppID.
   */
  async function openInSteam() {
    if (steamAppId.value === null) return;
    await invoke("steam_open_shortcut", { appId: steamAppId.value });
  }

  return { playtime, steamAppId, refresh, openInSteam };
};
