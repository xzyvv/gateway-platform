<template>
  <div class="section-toggle">
    <el-switch
      :model-value="ntp !== null"
      @update:model-value="(val: boolean) => (ntp = val ? { server: 'ntp.aliyun.com', sync_interval: 3600 } : null)"
      active-text="启用 NTP"
      inactive-text="使用默认值"
    />
  </div>
  <el-form v-if="ntp" :model="ntp" label-width="140px" class="ntp-form">
    <el-form-item label="NTP 服务器">
      <el-input v-model="ntp.server" placeholder="ntp.aliyun.com">
        <template #prefix><el-icon><Clock /></el-icon></template>
      </el-input>
    </el-form-item>
    <el-form-item label="同步间隔">
      <!-- min/max 以「秒」为单位（60 秒 = 1 分钟，86400 秒 = 1 天），展示单位默认分钟 -->
      <DurationSelect v-model="ntp.sync_interval" unit="min" :min="60" :max="86400" :units="['min', 'h']" />
    </el-form-item>
  </el-form>
</template>

<script setup lang="ts">
import DurationSelect from '@/components/industrial/DurationSelect.vue'
import type { NtpConfig } from '@/api/types'

// null 表示「使用后端默认值」，未启用时不写入配置节
const ntp = defineModel<NtpConfig | null>({ required: true })
</script>