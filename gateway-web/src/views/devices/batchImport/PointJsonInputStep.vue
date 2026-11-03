<template>
  <div>
    <el-input
      v-model="text"
      type="textarea"
      :rows="12"
      placeholder='[{"id":"p1","name":"温度","bus":"bus1","protocol":"modbus","address":"40001","slave_id":1,"interval":10,"on_change":false}]'
      aria-label="待导入的点位 JSON"
    />
    <div class="tpl-hint json-hint">
      <p>粘贴点位 JSON 数组后点击「校验数据」</p>
      <ul>
        <li>必填字段：<b>id</b>、<b>bus</b>、<b>protocol</b>、<b>address</b>、<b>interval</b>、<b>on_change</b></li>
        <li><b>device_id</b> 若填写需与「设备管理」中的设备 ID 一致</li>
      </ul>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 批量导入 · 点位分支 · JSON 路径第 1 步（粘贴 JSON）。
 *
 * 只负责渲染文本域与字段说明，解析与校验由调用方在点击「校验数据」时执行。
 *
 * Models:
 *   - text: 待校验的点位 JSON 文本（v-model）
 *
 * 典型应用场景：批量导入页点位分支选择 JSON 方式后的第 1 步。
 */
const text = defineModel<string>('text', { required: true })
</script>

<style scoped>
.tpl-hint { margin-top: 16px; text-align: left; max-width: 500px; font-size: 0.8125rem; color: var(--text-secondary); }
.json-hint { max-width: none; }
.tpl-hint ul { padding-left: 20px; line-height: 1.8; }
.tpl-hint p { margin-bottom: 6px; font-weight: 600; color: var(--text-regular); }
</style>
