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

    <p v-else-if="status.launches.length === 0" class="text-sm text-zinc-400 italic">
      This game has no launch option that can be turned into a Steam shortcut.
    </p>

    <template v-else>
      <!-- Steam rewrites its shortcuts file on exit, so writing now would be
           silently undone. Say so instead of failing mysteriously. -->
      <div
        v-if="status.install.running"
        class="rounded-md bg-yellow-500/10 p-4 text-sm text-yellow-500"
      >
        Steam is running. Close it completely before adding or removing a
        shortcut &mdash; Steam rewrites its shortcuts file when it exits, which
        would undo the change.
      </div>

      <div v-if="status.install.users.length > 1">
        <label class="block text-sm font-medium text-zinc-100">Steam account</label>
        <select
          v-model="accountId"
          class="mt-2 block w-full rounded-md bg-white/5 py-1.5 px-3 text-sm text-white outline-1 -outline-offset-1 outline-white/10"
        >
          <option v-for="user in status.install.users" :key="user.accountId" :value="user.accountId">
            {{ user.persona ?? `Account ${user.accountId}` }}
          </option>
        </select>
      </div>

      <div v-if="status.launches.length > 1">
        <label class="block text-sm font-medium text-zinc-100">Which launcher</label>
        <select
          v-model="launchIndex"
          class="mt-2 block w-full rounded-md bg-white/5 py-1.5 px-3 text-sm text-white outline-1 -outline-offset-1 outline-white/10"
        >
          <option v-for="launch in status.launches" :key="launch.index" :value="launch.index">
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

      <div v-if="shortcut" class="rounded-md bg-green-500/10 p-4">
        <p class="text-sm text-green-500">
          Added to Steam as &ldquo;{{ shortcut.appName }}&rdquo;.
        </p>
        <p v-if="!shortcut.managedByDrop" class="mt-1 text-xs text-zinc-400">
          This shortcut was not created by Drop. Updating it will leave your own
          settings for it intact.
        </p>
        <p class="mt-1 text-xs text-zinc-400">
          Restart Steam if you do not see it yet.
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
          :disabled="busy || status.install.running"
          class="inline-flex justify-center rounded-md bg-zinc-800 px-3 py-2 text-sm font-semibold text-red-400 ring-1 ring-inset ring-zinc-700 hover:bg-zinc-900 disabled:opacity-50"
        >
          Remove from Steam
        </button>
      </div>

      <div v-if="error" class="rounded-md bg-red-600/10 p-4">
        <p class="text-sm font-medium text-red-600">{{ error }}</p>
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
type SteamGameStatus = {
  install: SteamInstall | null;
  shortcut: ShortcutRecord | null;
  launches: ResolvedLaunch[];
};

const props = defineProps<{ gameId: string }>();
const game = await useGame(props.gameId);
const appName = game.game.mName;

const status = ref<SteamGameStatus>(
  await invoke<SteamGameStatus>("steam_game_status", {
    gameId: props.gameId,
    appName,
    accountId: null,
  }),
);

// Users arrive most-recently-used first, so the head is the sensible default.
const accountId = ref<number | null>(status.value.install?.users[0]?.accountId ?? null);
const launchIndex = ref<number>(status.value.launches[0]?.index ?? 0);
const shortcut = ref<ShortcutRecord | null>(status.value.shortcut);
const busy = ref(false);
const error = ref<string | undefined>();

const selectedLaunch = computed(() =>
  status.value.launches.find((l) => l.index === launchIndex.value),
);

const canWrite = computed(
  () =>
    !busy.value &&
    !status.value.install?.running &&
    (selectedLaunch.value?.exists ?? false),
);

async function refresh() {
  status.value = await invoke<SteamGameStatus>("steam_game_status", {
    gameId: props.gameId,
    appName,
    accountId: accountId.value,
  });
  shortcut.value = status.value.shortcut;
}

async function run(action: () => Promise<void>) {
  busy.value = true;
  error.value = undefined;
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
    shortcut.value = await invoke<ShortcutRecord>("steam_add_shortcut", {
      gameId: props.gameId,
      appName,
      launchIndex: launchIndex.value,
      accountId: accountId.value,
    });
  });

const removeFromSteam = () =>
  run(async () => {
    if (!shortcut.value) return;
    await invoke("steam_remove_shortcut", {
      appId: shortcut.value.appId,
      accountId: accountId.value,
    });
    shortcut.value = null;
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

// Switching account changes which shortcuts file we are looking at.
watch(accountId, () => {
  refresh().catch((e) => (error.value = (e as unknown as string).toString()));
});
</script>
