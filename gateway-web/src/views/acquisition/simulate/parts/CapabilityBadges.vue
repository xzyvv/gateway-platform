<template>
  <!-- 协议能力徽标：由 GET /api/protocols 动态声明，随所选设备协议变化。
       能力元数据缺失（未加载/未知协议）时显示「能力未知」，不谎称只读 -->
  <div class="cap-badges">
    <el-tag size="small" effect="plain" :type="capsKnown ? (supportsWrite ? 'success' : 'info') : 'warning'">
      {{ !capsKnown ? '写入能力未知' : supportsWrite ? '支持写入' : '只读' }}
    </el-tag>
    <el-tag size="small" effect="plain" :type="capsKnown ? (supportsBatchRead ? 'success' : 'info') : 'warning'">
      {{ !capsKnown ? '读取能力未知' : supportsBatchRead ? '支持批量读' : '单点读' }}
    </el-tag>
    <el-tag size="small" effect="plain" :type="supportsScanSlaves ? 'success' : 'info'">
      {{ supportsScanSlaves ? '支持扫描从站' : '不支持扫描' }}
    </el-tag>
    <el-tag size="small" effect="plain" :type="supportsDiag ? 'success' : 'info'">
      {{ supportsDiag ? '支持协议诊断' : '无协议诊断' }}
    </el-tag>
  </div>
</template>

<script setup lang="ts">
/**
 * 协议能力徽标（从 Tester.vue 拆出）
 *
 * 职责：把 GET /api/protocols 返回的能力声明渲染成一行小标签，
 * 让现场一眼看出当前协议能不能写、能不能批量读、能不能扫描、能不能诊断。
 *
 * 能力门控口径：「能力未知」不等于「能力具备」——元数据缺失时统一按未知展示，
 * 由父组件把每个能力位保守地按 false 处理（见 Tester.vue 的 supports* 计算属性）。
 *
 * Props:
 * - capsKnown：所选协议的能力元数据是否已加载（false = 能力未知）
 * - supportsWrite / supportsBatchRead / supportsScanSlaves / supportsDiag：各能力位声明
 *
 * Emits: 无（纯展示）
 *
 * 典型应用场景：点位读写测试页（Tester.vue）的设备选择器下方。
 */
defineProps<{
  /** 所选协议的能力元数据是否已知 */
  capsKnown: boolean
  /** 是否支持写入 */
  supportsWrite: boolean
  /** 是否支持批量读 */
  supportsBatchRead: boolean
  /** 是否支持扫描从站 */
  supportsScanSlaves: boolean
  /** 是否支持协议诊断 */
  supportsDiag: boolean
}>()
</script>

<style scoped>
/* 以下规则来自原 Tester.css，随区块一起迁移到本组件 */
/* 协议能力徽标：设备选择器下方的小标签行 */
.cap-badges {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 8px;
}
</style>
