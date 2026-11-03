<template>
  <!--
    全站品牌标识（登录页 + 侧栏共用，避免两处造型不一致）
    造型：双层六边形（纵深）+ 汇聚核心（圆环+实心芯）+ 三条数据链路与终端节点，
    表达「多协议终端汇聚到一个网关」的语义。
    颜色统一用 currentColor：登录页在品牌渐变底上取反色，侧栏随主题色，无需改图。
  -->
  <svg
    class="brand-mark"
    :width="size"
    :height="size"
    viewBox="0 0 48 48"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    role="img"
    aria-label="工业网关"
  >
    <!-- 外六边形：弱描边，做层次 -->
    <path d="M24 3.4 42.6 14.2v19.6L24 44.6 5.4 33.8V14.2z" stroke-opacity="0.28" />
    <!-- 内六边形：主体轮廓 -->
    <path d="M24 9.6 37.2 17.3v15.4L24 40.4 10.8 32.7V17.3z" stroke-opacity="0.9" stroke-width="1.8" />
    <!-- 汇聚核心 -->
    <circle cx="24" cy="24" r="4.4" stroke-width="1.8" />
    <circle cx="24" cy="24" r="1.6" fill="currentColor" stroke="none" />
    <!-- 三条链路 + 终端节点 -->
    <path d="M24 19.6v-5.9" stroke-width="1.6" />
    <path d="m20.4 26.2-4.9 2.8" stroke-width="1.6" />
    <path d="m27.6 26.2 4.9 2.8" stroke-width="1.6" />
    <circle cx="24" cy="11.9" r="1.8" fill="currentColor" stroke="none" />
    <circle cx="13" cy="30.6" r="1.8" fill="currentColor" stroke="none" />
    <circle cx="35" cy="30.6" r="1.8" fill="currentColor" stroke="none" />
  </svg>
</template>

<script setup lang="ts">
withDefaults(defineProps<{
  /** 边长（px），默认 40 */
  size?: number
}>(), { size: 40 })
</script>

<style scoped>
.brand-mark {
  display: block;
  shape-rendering: geometricPrecision;
}
</style>
