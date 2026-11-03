<template>
  <!-- HEX + ASCII 双栏对照（审查#7）：左偏移、中 HEX（16 字节时 8+8 分组）、右 ASCII，等宽对齐 -->
  <div class="hex-ascii" role="table" :aria-label="ariaLabel">
    <div v-for="r in rows" :key="r.offset" class="ha-row" role="row">
      <span class="ha-offset" role="cell">{{ r.offset }}</span>
      <span class="ha-hex" role="cell"><span
        v-for="(b, i) in r.bytes"
        :key="i"
        class="ha-byte"
        :class="{ 'ha-gap': bytesPerRow === 16 && i === 8 }"
      >{{ b }}</span></span>
      <span class="ha-ascii" role="cell">{{ r.ascii }}</span>
    </div>
    <div v-if="!rows.length" class="ha-empty">（空报文）</div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { hexDumpRows } from '@/utils/hexDump'

const props = withDefaults(defineProps<{ hex: string; bytesPerRow?: number; ariaLabel?: string }>(), {
  bytesPerRow: 16,
  ariaLabel: '报文 HEX 与 ASCII 对照',
})
const rows = computed(() => hexDumpRows(props.hex, props.bytesPerRow))
</script>

<style scoped>
.hex-ascii { background: var(--el-fill-color-light); padding: 12px; border-radius: var(--radius-button); font-family: monospace; font-size: 0.8125rem; overflow-x: auto; }
.ha-row { display: flex; white-space: pre; line-height: 1.7; }
.ha-offset { color: var(--el-text-color-secondary); margin-right: 12px; user-select: none; }
.ha-hex { flex: 1; }
.ha-byte { margin-right: 6px; color: var(--el-color-primary); }
.ha-gap { margin-left: 12px; }
.ha-ascii { margin-left: 12px; padding-left: 12px; border-left: 1px solid var(--el-border-color); color: var(--el-text-color-regular); }
.ha-empty { color: var(--el-text-color-secondary); }
</style>
