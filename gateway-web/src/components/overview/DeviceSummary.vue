<template>
  <el-card shadow="never" class="section-card">
    <template #header>
      <SectionHeader title="设备概览" icon="Cpu">
        <template #extra>
          <el-tag type="info" effect="plain" round size="small" class="count-badge">
            {{ deviceList.length }} 台
          </el-tag>
        </template>
      </SectionHeader>
    </template>
    <div v-if="deviceList.length === 0" class="empty-devices">
      <empty-state description="暂无设备，请先到设备管理中新增" :image-size="80" />
    </div>
    <div v-else class="device-grid">
      <div
        v-for="d in deviceList"
        :key="d.id"
        class="device-card"
        role="button"
        tabindex="0"
        @click="emit('device-click', d)"
        @keydown.enter.prevent="emit('device-click', d)"
        @keydown.space.prevent="emit('device-click', d)"
      >
        <div class="device-card-header">
          <div class="device-title">
            <span
              class="device-status-dot"
              :class="(d as Record<string, unknown>).enabled !== false ? 'online' : 'offline'"
              role="img"
              :aria-label="(d as Record<string, unknown>).enabled !== false ? '在线' : '离线'"
            ></span>
            <span class="device-name" :title="d.name">{{ d.name }}</span>
          </div>
          <el-tag :type="protocolType(d.protocol)" size="small" effect="dark" round>{{ protocolLabel[d.protocol] || d.protocol }}</el-tag>
        </div>
        <div class="device-info">
          <div class="info-row">
            <el-icon><Connection /></el-icon>
            <span class="info-label">总线</span>
            <span class="info-value">{{ d.bus }}</span>
          </div>
          <div class="info-row">
            <el-icon><Location /></el-icon>
            <span class="info-label">地址</span>
            <span class="info-value">{{ d.address }}</span>
          </div>
          <div class="info-row" v-if="d.template">
            <el-icon><Collection /></el-icon>
            <span class="info-label">模板</span>
            <span class="info-value ellipsis" :title="d.template">{{ d.template }}</span>
          </div>
        </div>
      </div>
    </div>
  </el-card>
</template>

<script setup lang="ts">
import { Connection, Location, Collection } from '@element-plus/icons-vue'
import type { DeviceConfig } from '@/api/types'
import { protocolLabel, protocolType } from '@/composables/devices/useProtocol'

defineProps<{ deviceList: DeviceConfig[] }>()
const emit = defineEmits<{ 'device-click': [d: DeviceConfig] }>()
</script>
