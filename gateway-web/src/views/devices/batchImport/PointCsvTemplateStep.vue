<template>
  <div>
    <EmptyState description="下载 CSV 模板，按格式填写点位信息后上传" :image-size="80" />
    <el-button type="primary" @click="emit('download-template')">
      <el-icon><Download /></el-icon> 下载 CSV 模板
    </el-button>
    <el-button @click="emit('download-full-template')">
      <el-icon><Download /></el-icon> 下载完整字段模板
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
        <li><b>slave_id</b>（从站地址）Modbus 从站地址（modbus 必填）</li>
        <li><b>address_num</b>（地址偏移，0 基寄存器偏移）选填，默认 0</li>
        <li><b>ct_ratio</b>（CT 变比，影响电流/功率/电量）选填，留空=不启用，填了须 &gt; 0</li>
        <li><b>pt_ratio</b>（PT 变比，影响电压/功率/电量）选填，留空=不启用，填了须 &gt; 0</li>
      </ul>
      <p class="tpl-extra">完整字段模板另有 scale、offset、unit、公式、告警阈值、可写标记等列，
        与「批量导出」的完整档列名一致。导入时默认按「合并」语义写入：CSV 里出现的列覆盖现有点位，
        没出现的列保留原值，出现的列留空表示清空该字段。</p>
      <p class="tpl-extra">unit 建议用规范写法（V、kWh、m3/h 等），点位表单里只能从单位下拉清单中选，
        同义写法（如 KWH、m³）会让上行分组出现重复维度。</p>
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
 *   - download-template: 点击「下载 CSV 模板」（精简 10 列）
 *   - download-full-template: 点击「下载完整字段模板」（全部可平铺字段）
 *
 * 典型应用场景：批量导入页点位分支选择 CSV 方式后的第 1 步。
 */
import { Download } from '@element-plus/icons-vue'
import EmptyState from '@/components/EmptyState.vue'

const emit = defineEmits<{
  'download-template': []
  'download-full-template': []
}>()
</script>

<style scoped>
.tpl-hint { margin-top: 16px; text-align: left; max-width: 500px; font-size: 0.8125rem; color: var(--text-secondary); }
.tpl-hint ul { padding-left: 20px; line-height: 1.8; }
.tpl-hint p { margin-bottom: 6px; font-weight: 600; color: var(--text-regular); }
.tpl-extra { font-weight: 400 !important; line-height: 1.7; margin-top: 10px !important; }
</style>
