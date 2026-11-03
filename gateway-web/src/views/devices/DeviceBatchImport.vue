<template>
  <div class="import">
    <SectionHeader title="批量导入" icon="Upload">
      <template #extra>
        <el-radio-group v-model="target" size="small" aria-label="导入对象">
          <el-radio-button value="device">设备</el-radio-button>
          <el-radio-button value="point">点位</el-radio-button>
        </el-radio-group>
      </template>
    </SectionHeader>

    <!-- ========== 设备分支（原 DeviceBatchImport 行为保持不变） ========== -->
    <DeviceImportBranch v-if="target === 'device'" :templates="templates" />

    <!-- ========== 点位分支（原 PointBatchImport 能力已并入） ========== -->
    <PointImportBranch v-else />
  </div>
</template>

<script setup lang="ts">
/**
 * 批量导入（设备 / 点位合并页）。
 *
 * 合并来源：原「点位 CSV 批量导入」页 PointBatchImport.vue 已删除，其 CSV 模板下载、
 * 上传解析校验预览、确认导入以及 JSON 粘贴导入能力全部并入本页，通过顶部「导入对象」
 * 切换走设备或点位分支。点位导入现在从这里进（/devices?tab=batch-import），
 * 点位 Hub 不再保留 batch-import 页签。
 *
 * 两条分支共用 el-steps 骨架但状态各自独立（设备沿用原 step/result/outcomes，
 * 点位用 pointStep/pointResult/pointPayload），切换对象互不污染；
 * 服务端契约不变：设备逐台调用 createDevice，点位调用 batchImportPoints。
 *
 * 结构：设备分支与点位分支各拆一个子组件（见 `./batchImport/`），本页只保留
 * 「导入对象」切换与两条分支共用的网关模板库。
 */
import { ref, onMounted } from 'vue'
import { listTemplates } from '@/api/templates'
import type { TemplateInfo } from '@/utils/batchImportCsv'
import SectionHeader from '@/components/SectionHeader.vue'
import DeviceImportBranch from './batchImport/DeviceImportBranch.vue'
import PointImportBranch from './batchImport/PointImportBranch.vue'

/** 导入对象：设备 / 点位 */
const target = ref<'device' | 'point'>('device')

/** 网关模板库：用于本地校验「模板存在」与「模板协议 = 设备协议」，拉取失败时留空交给后端兜底 */
const templates = ref<TemplateInfo[]>([])

onMounted(async () => {
  try {
    const list = await listTemplates()
    templates.value = list.map((t) => ({ name: t.name, protocol: t.protocol }))
  } catch {
    // 模板库拉取失败不阻断导入：本地跳过模板校验，由后端 create_device 兜底并返回原因
    templates.value = []
  }
})
</script>

<style scoped>
.import { padding: 0px; }
</style>
