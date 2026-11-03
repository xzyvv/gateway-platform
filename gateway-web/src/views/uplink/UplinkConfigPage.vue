<template>
  <div class="uplink-config-page">
    <!-- C3 术语收口：本页两段读的是同一份 /api/uplink/config，但职责不同——
         「通道转发」决定数据发往哪里（目标/周期/启停/连通性测试），
         「上送模板」决定数据长什么样（模板内容/编码/字节上限）。
         原「上行通道」/「上行通道参数」两名极易混淆，统一改名后各归其位。 -->
    <UplinkForwardConfig />
    <UplinkConfig />
  </div>
</template>

<script setup lang="ts">
import UplinkForwardConfig from './UplinkForwardConfig.vue'
import UplinkConfig from './UplinkConfig.vue'
</script>

<style scoped>
/* 两段各有一张 section-card，留出间距避免卡片贴在一起 */
.uplink-config-page {
  display: flex;
  flex-direction: column;
  gap: 16px;
}
</style>
