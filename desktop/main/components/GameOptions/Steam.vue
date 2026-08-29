<!--
  ZOUGCLOUD(ZC-004/005/006): optional Steam integration.

  Drop stays the installer and updater; Steam optionally becomes the launcher so
  members get the overlay, controller support and playtime tracking. The
  shortcut always points at the game executable, never at Drop, or Steam would
  record playtime for the launcher instead of the game.
-->
<template>
  <div class="space-y-6">
    <div>
      <h3 class="text-sm font-medium leading-6 text-zinc-100">Steam</h3>
      <p class="mt-1 text-sm leading-6 text-zinc-400">
        Add this game to Steam as a non-Steam shortcut. Drop keeps managing
        installs and updates.
      </p>
    </div>

    <!-- Steam absent: hide the feature rather than offer something inert. -->
    <p v-if="!status.install" class="text-sm text-zinc-400 italic">
      Steam was not found on this computer.
    </p>

    <p
      v-else-if="status.launches.length === 0"
      class="text-sm text-zinc-400 italic"
    >
      This game has no launch option that can be turned into a Steam shortcut.
    </p>

    <template v-else>
      <div v-if="status.install.users.length > 1">
        <label class="block text-sm font-medium text-zinc-100"
          >Steam account</label
        >
        <select
          v-model="accountId"
          class="mt-2 block w-full rounded-md bg-white/5 py-1.5 px-3 text-sm text-white outline-1 -outline-offset-1 outline-white/10"
        >
          <option
            v-for="user in status.install.users"
            :key="user.accountId"
            :value="user.accountId"
          >
            {{ user.persona ?? `Account ${user.accountId}` }}
          </option>
        </select>
      </div>

      <div v-if="status.launches.length > 1">
        <label class="block text-sm font-medium text-zinc-100"
          >Which launcher</label
        >
        <select
          v-model="launchIndex"
          class="mt-2 block w-full rounded-md bg-white/5 py-1.5 px-3 text-sm text-white outline-1 -outline-offset-1 outline-white/10"
        >
          <option
            v-for="launch in status.launches"
            :key="launch.index"
            :value="launch.index"
          >
            {{ launch.name }}
          </option>
        </select>
      </div>

      <p v-if="selectedLaunch" class="text-xs text-zinc-500 break-all">
        Steam will run: {{ selectedLaunch.exe }}
        <span v-if="!selectedLaunch.exists" class="text-red-500">
          &mdash; this file is missing
        </span>
      </p>

      <!-- Steam only reads shortcuts.vdf and the artwork folder at startup, and
           rewrites the former from memory on exit. So we close it, write, and
           start it again rather than making the member work that out. -->
      <p v-if="status.install.running" class="text-xs text-zinc-400">
        Steam is running and will be closed and restarted automatically. Quit any
        game running through Steam first, or it will refuse to close.
      </p>

      <div v-if="shortcut" class="rounded-md bg-green-500/10 p-4">
        <p class="text-sm text-green-500">
          Added to Steam as &ldquo;{{ shortcut.appName }}&rdquo;.
        </p>
        <p class="mt-1 text-xs text-zinc-400">
          Artwork:
          <span v-if="artwork.length">{{ artworkLabel }}</span>
          <span v-else class="italic">none yet</span>
        </p>
        <p v-if="!shortcut.managedByDrop" class="mt-1 text-xs text-zinc-400">
          This shortcut was not created by Drop. Updating it leaves your own
          settings for it intact.
        </p>
      </div>

      <div class="flex flex-row flex-wrap gap-2">
        <LoadingButton
          @click="() => addToSteam()"
          :loading="busy"
          :disabled="!canWrite"
          class="w-fit"
        >
          {{ shortcut ? "Update shortcut" : "Add to Steam" }}
        </LoadingButton>

        <button
          v-if="shortcut"
          @click="() => openInSteam()"
          type="button"
          class="inline-flex justify-center rounded-md bg-zinc-800 px-3 py-2 text-sm font-semibold text-zinc-100 ring-1 ring-inset ring-zinc-700 hover:bg-zinc-900"
        >
          Open in Steam
        </button>

        <button
          v-if="shortcut"
          @click="() => removeFromSteam()"
          type="button"
          :disabled="busy"
          class="inline-flex justify-center rounded-md bg-zinc-800 px-3 py-2 text-sm font-semibold text-red-400 ring-1 ring-inset ring-zinc-700 hover:bg-zinc-900 disabled:opacity-50"
        >
          Remove from Steam
        </button>
      </div>

      <p v-if="busy" class="text-xs text-zinc-400">
        {{
          status.install.running
            ? "Closing Steam, writing the shortcut and fetching artwork…"
            : "Writing the shortcut and fetching artwork…"
        }}
      </p>

      <div v-if="notice" class="rounded-md bg-blue-500/10 p-4">
        <p class="text-sm text-blue-400">{{ notice }}</p>
      </div>

      <div v-if="error" class="rounded-md bg-red-600/10 p-4">
        <p class="text-sm font-medium text-red-600">{{ error }}</p>
      </div>

      <!-- ZC-005: optional artwork source. Works without a key. -->
      <div class="border-t border-zinc-800 pt-6">
        <h4 class="text-sm font-medium text-zinc-100">SteamGridDB</h4>
        <p class="mt-1 text-sm text-zinc-400">
          Optional. With a personal API key, Drop pulls proper Steam artwork
          (capsule, hero, logo, icon). Without one it falls back to this game's
          own Drop images, so the shortcut is never blank.
        </p>
        <div class="mt-3 flex flex-row gap-2">
          <input
            v-model="gridKeyInput"
            type="password"
            :placeholder="
              status.steamgriddbConfigured ? 'A key is saved' : 'Paste your API key'
            "
            class="block w-full rounded-md bg-white/5 py-1.5 px-3 text-sm text-white outline-1 -outline-offset-1 outline-white/10"
          />
          <button
            @click="() => saveGridKey()"
            type="button"
            class="shrink-0 inline-flex justify-center rounded-md bg-zinc-800 px-3 py-2 text-sm font-semibold text-zinc-100 ring-1 ring-inset ring-zinc-700 hover:bg-zinc-900"
          >
            Save
          </button>
          <button
            v-if="status.steamgriddbConfigured"
            @click="() => clearGridKey()"
            type="button"
            class="shrink-0 inline-flex justify-center rounded-md bg-zinc-800 px-3 py-2 text-sm font-semibold text-red-400 ring-1 ring-inset ring-zinc-700 hover:bg-zinc-900"
          >
            Forget
          </button>
        </div>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";

// The modal passes its shared configuration model to every tab; this one does
// not edit the game configuration, so it is not bound.
defineOptions({ inheritAttrs: false });

type SteamUser = {
  accountId: number;
  persona: string | null;
  mostRecent: boolean;
  userdataDir: string;
};
type SteamInstall = { path: string; users: SteamUser[]; running: boolean };
type ShortcutRecord = {
  appId: number;
  appName: string;
  exe: string;
  startDir: string;
  launchOptions: string;
  runGameId: string;
  managedByDrop: boolean;
};
type ResolvedLaunch = {
  index: number;
  name: string;
  exe: string;
  args: string[];
  workingDir: string;
  exists: boolean;
};
type ArtworkKind = "capsule" | "portrait" | "hero" | "logo" | "icon";
type SteamGameStatus = {
  install: SteamInstall | null;
  shortcut: ShortcutRecord | null;
  launches: ResolvedLaunch[];
  artwork: ArtworkKind[];
  steamgriddbConfigured: boolean;
};
type AddShortcutOutcome = {
  shortcut: ShortcutRecord;
  artwork: ArtworkKind[];
  steamRestarted: boolean;
};

const props = defineProps<{ gameId: string }>();
const game = await useGame(props.gameId);
const appName = game.game.mName;

// Drop's own images, used when SteamGridDB has nothing or no key is set.
const dropArtwork = {
  coverObjectId: game.game.mCoverObjectId || null,
  bannerObjectId: game.game.mBannerObjectId || null,
  iconObjectId: game.game.mIconObjectId || null,
};

const status = ref<SteamGameStatus>(
  await invoke<SteamGameStatus>("steam_game_status", {
    gameId: props.gameId,
    appName,
    accountId: null,
  }),
);

// Users arrive most-recently-used first, so the head is the sensible default.
const accountId = ref<number | null>(
  status.value.install?.users[0]?.accountId ?? null,
);
const launchIndex = ref<number>(status.value.launches[0]?.index ?? 0);
const shortcut = ref<ShortcutRecord | null>(status.value.shortcut);
const artwork = ref<ArtworkKind[]>(status.value.artwork);
const busy = ref(false);
const error = ref<string | undefined>();
const notice = ref<string | undefined>();
const gridKeyInput = ref("");

const selectedLaunch = computed(() =>
  status.value.launches.find((l) => l.index === launchIndex.value),
);

const canWrite = computed(
  () => !busy.value && (selectedLaunch.value?.exists ?? false),
);

const artworkLabel = computed(() => artwork.value.join(", "));

async function refresh() {
  status.value = await invoke<SteamGameStatus>("steam_game_status", {
    gameId: props.gameId,
    appName,
    accountId: accountId.value,
  });
  shortcut.value = status.value.shortcut;
  artwork.value = status.value.artwork;
}

async function run(action: () => Promise<void>) {
  busy.value = true;
  error.value = undefined;
  notice.value = undefined;
  try {
    await action();
    await refresh();
  } catch (e) {
    error.value = (e as unknown as string).toString();
  }
  busy.value = false;
}

const addToSteam = () =>
  run(async () => {
    const outcome = await invoke<AddShortcutOutcome>("steam_add_shortcut", {
      gameId: props.gameId,
      appName,
      launchIndex: launchIndex.value,
      accountId: accountId.value,
      dropArtwork,
    });
    shortcut.value = outcome.shortcut;
    artwork.value = outcome.artwork;
    notice.value = outcome.steamRestarted
      ? "Done. Steam was closed and restarted, so the shortcut should already be there."
      : "Done. Start Steam to see the shortcut.";
  });

const removeFromSteam = () =>
  run(async () => {
    if (!shortcut.value) return;
    await invoke("steam_remove_shortcut", {
      appId: shortcut.value.appId,
      accountId: accountId.value,
    });
    shortcut.value = null;
    artwork.value = [];
    notice.value = "Removed from Steam.";
  });

async function openInSteam() {
  if (!shortcut.value) return;
  error.value = undefined;
  try {
    await invoke("steam_open_shortcut", { appId: shortcut.value.appId });
  } catch (e) {
    error.value = (e as unknown as string).toString();
  }
}

async function saveGridKey() {
  await run(async () => {
    await invoke("steam_set_steamgriddb_key", { key: gridKeyInput.value });
    gridKeyInput.value = "";
    notice.value = "SteamGridDB key saved.";
  });
}

async function clearGridKey() {
  await run(async () => {
    await invoke("steam_set_steamgriddb_key", { key: null });
    gridKeyInput.value = "";
    notice.value = "SteamGridDB key forgotten.";
  });
}

// Switching account changes which shortcuts file we are looking at.
watch(accountId, () => {
  refresh().catch((e) => (error.value = (e as unknown as string).toString()));
});
</script>
