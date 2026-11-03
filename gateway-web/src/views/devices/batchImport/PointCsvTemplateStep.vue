<template>
  <div>
    <EmptyState description="下载 CSV 模板，按格式填写点位信息后上传" :image-size="80" />
    <el-button type="primary" @click="emit('download-template')">
      <el-icon><Download /></el-icon> 下载 CSV 模板
    </el-button>
    <div class="tpl-hint">
      <p>CSV 文件格式字段说明如下</p>
      <ul>
        <li><b>id</b>（点位 ID）唯一标识符（必填）</li>
        <li><b>name</b>（名称）点位名称（必填）</li>
        <li><b>device_id</b>（所属设备）需先在「设备管理」创建（必填）</li>
        <li><b>bus</b>（总线）所属总线（必填）</li>
        <li><b>protocol</b>（协议）modbus、dlt645、cjt188 等（必填）</li>
        <li><b>address</b>（地址）寄存器地址 / 通信标识（必填）</li>
        <li><b>slave_id</b>（从站号）Modbus 从站 ID（modbus 必填）</li>
        <li><b>address_num</b>（读取数量）选填，默认 1</li>
      </ul>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 批量导入 · 点位分支 · CSV 路径第 1 步（下载模板 + 字段说明）。
 *
 * 只负责渲染模板下载入口与 CSV 字段说明，模板内容由调用方生成。
 *
 * Emits:
 *   - download-template: 点击「下载 CSV 模板」
 *
 * 典型应用场景：批量导入页点位分支选择 CSV 方式后的第 1 步。
 */
import { Download } from '@element-plus/icons-vue'
import EmptyState from '@/components/EmptyState.vue'

const emit = defineEmits<{
  'download-template': []
}>()
</script>

<style scoped>
.tpl-hint { margin-top: 16px; text-align: left; max-width: 500px; font-size: 0.8125rem; color: var(--text-secondary); }
.tpl-hint ul { padding-left: 20px; line-height: 1.8; }
.tpl-hint p { margin-bottom: 6px; font-weight: 600; color: var(--text-regular); }
</style>
