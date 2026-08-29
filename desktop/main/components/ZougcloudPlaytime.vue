<!--
  ZOUGCLOUD(ZC-008): local playtime, shown beside the update status.

  A separate component so the two status branches on the game page share it and
  our patch stays a one-line insertion in an upstream file.

  Renders nothing when the game has never been played: the backend returns a
  null `display` in that case rather than "0 min", so there is no empty bullet
  left on the line.

  English only, like the rest of Drop Desktop, which has no i18n.
-->
<template>
  <template v-if="playtime?.display">
    <span aria-hidden="true" class="text-zinc-600">&bull;</span>
    <span :title="playtime.lastPlayedDisplay ?? undefined">
      {{ playtime.display }}
    </span>
  </template>
</template>

<script setup lang="ts">
import type { PlaytimeSummary } from "~/composables/zougcloud";

defineProps<{ playtime: PlaytimeSummary | null }>();
</script>
