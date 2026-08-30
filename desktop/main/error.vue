<!--
  ZOUGCLOUD(ZC-012): recoverable error page.

  Upstream's version had two defects that turned any error into a dead end:

  1. Its only way out was `<a href="/store">` — a raw href. The app is served
     with baseURL /main, so that resolves outside the app and produces another
     asset error. Recovering meant mouse-Back then F5, which no user should
     have to know.

  2. It rendered inside `NuxtLayout default`, which mounts the header, which
     awaits the user object and a Tauri command. An error caused by bad app
     state therefore risked failing the error page too.

  This version has no layout and no data dependencies, and recovers through
  `clearError({ redirect })` so the transient error state is actually cleared
  rather than navigated around.
-->
<template>
  <div
    class="min-h-screen w-full flex flex-col items-center justify-center bg-zinc-900 px-6 text-center"
  >
    <Logo class="h-10 w-auto mb-10 opacity-80" />

    <h1 class="text-3xl sm:text-4xl font-bold font-display text-zinc-100">
      {{ title }}
    </h1>

    <p class="mt-4 max-w-md text-base leading-7 text-zinc-400">
      {{ description }}
    </p>

    <div class="mt-10 flex flex-col sm:flex-row items-center gap-3">
      <button
        @click="() => backToLibrary()"
        type="button"
        class="transition-transform duration-300 hover:scale-105 active:scale-95 inline-flex items-center justify-center rounded-md bg-blue-600 px-6 py-3 font-semibold text-white shadow-xl hover:bg-blue-700 uppercase font-display"
      >
        Back to library
      </button>

      <!-- Deliberately absent for a route that does not exist: retrying it
           would fail identically and invite an error loop. -->
      <button
        v-if="canRetry"
        @click="() => retry()"
        type="button"
        class="transition inline-flex items-center justify-center rounded-md bg-zinc-800 px-6 py-3 font-semibold text-zinc-100 ring-1 ring-inset ring-zinc-700 hover:bg-zinc-700 uppercase font-display"
      >
        Retry
      </button>
    </div>

    <p v-if="detail" class="mt-10 max-w-lg text-xs text-zinc-600 break-words">
      {{ detail }}
    </p>
  </div>
</template>

<script setup lang="ts">
import type { NuxtError } from "#app";
import { classifyError, ErrorKind } from "~/composables/zougcloud-errors";

const props = defineProps({
  error: Object as () => NuxtError,
});

const kind = computed(() => classifyError(props.error));

const title = computed(() => {
  switch (kind.value) {
    case ErrorKind.NotFound:
      return "Page not found";
    case ErrorKind.ServerUnavailable:
      return "ZougCloud server unavailable";
    default:
      return "Something went wrong";
  }
});

const description = computed(() => {
  switch (kind.value) {
    case ErrorKind.NotFound:
      return "That page doesn't exist in Drop.";
    case ErrorKind.ServerUnavailable:
      return "Drop couldn't reach the ZougCloud server.";
    default:
      return "Drop couldn't load this page.";
  }
});

// A missing route cannot succeed on a second attempt, so offering Retry there
// would only walk the user into the same error again.
const canRetry = computed(() => kind.value !== ErrorKind.NotFound);

const detail = computed(() =>
  props.error?.statusMessage || props.error?.message || undefined,
);

/**
 * One click back to a working Library.
 *
 * `clearError` is what makes this a real recovery rather than a navigation:
 * it drops Nuxt's error state and then routes, so the app is not left holding
 * the broken state that caused the error.
 */
async function backToLibrary() {
  await clearError({ redirect: "/library" });
}

async function retry() {
  // Re-enter the route that failed. Falls back to the library rather than
  // risking a reload into the same broken state.
  const target = props.error?.url && props.error.url !== "/" ? props.error.url : "/library";
  await clearError({ redirect: target });
}

console.error(props.error);
</script>
