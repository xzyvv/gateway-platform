<template>
  <!-- 写入区：仅当协议声明 supports_write 时显示（能力来自 GET /api/protocols） -->
  <div class="write-section" v-if="hasPoint && supportsWrite">
    <el-divider />
    <SectionHeader title="写入测试" icon="Edit" />
    <div class="write-row">
      <el-input-number
        v-model="writeValue"
        :controls="false"
        placeholder="输入要写入值"
        aria-label="写入值"
        style="flex: 1"
        :step="0.01"
      />
      <el-button
        type="warning"
        :loading="writeLoading"
        @click="emit('write')"
      >
        <el-icon><Check /></el-icon> 写入
      </el-button>
    </div>
    <div class="write-tip">写入将直接下发到设备，请谨慎操作。</div>
  </div>
  <!-- 协议声明为只读时给出明确提示，避免用户找不到写入入口而困惑。
       能力元数据缺失时不谎称「只读」，而是提示能力未知（可能只是元数据未加载） -->
  <div class="write-section" v-else-if="hasPoint && !supportsWrite">
    <el-divider />
    <div class="write-tip">
      {{ capsKnown
        ? `当前设备协议（${selectedProtocol}）声明为只读，不支持写入测试。`
        : `当前设备协议（${selectedProtocol}）的写入能力未知，已保守隐藏写入入口；请检查协议能力元数据是否加载成功。` }}
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 写入测试区（从 Tester.vue 拆出）
 *
 * 职责：按协议能力声明决定「显示写入入口」还是「说明为什么不能写」，
 * 并收集待写入的数值交给父组件下发（真正的下发与二次确认在父组件）。
 *
 * Props:
 * - hasPoint：是否已选择点位（未选点位时整块不渲染）
 * - supportsWrite：协议是否声明 supports_write（能力未知时按 false 处理）
 * - capsKnown：能力元数据是否已知（决定提示文案是「只读」还是「能力未知」）
 * - selectedProtocol：所选设备对应的协议名（用于提示文案）
 * - writeLoading：写入请求进行中
 *
 * Emits:
 * - write：点击「写入」
 *
 * 典型应用场景：点位读写测试页（Tester.vue）的写入区（协议支持写入时才出现）。
 */
import { Check } from '@element-plus/icons-vue'

defineProps<{
  /** 是否已选择点位 */
  hasPoint: boolean
  /** 协议是否声明支持写入 */
  supportsWrite: boolean
  /** 能力元数据是否已知 */
  capsKnown: boolean
  /** 所选设备对应的协议名 */
  selectedProtocol: string
  /** 写入请求进行中 */
  writeLoading: boolean
}>()

/** 待写入的数值（与父组件双向绑定） */
const writeValue = defineModel<number | null>('writeValue', { required: true })

const emit = defineEmits<{
  /** 提交一次写入 */
  (e: 'write'): void
}>()
</script>

<style scoped>
/* 以下规则来自原 Tester.css，随区块一起迁移到本组件 */
.write-section {
  margin-top: 8px;
}
.write-row {
  display: flex;
  gap: 8px;
  align-items: center;
}
/* 窄屏下输入与按钮垂直堆叠，按钮占满整行便于点击 */
@media (max-width: 768px) {
  .write-row {
    flex-direction: column;
    align-items: stretch;
  }
}
.write-tip {
  font-size: 0.75rem;
  color: var(--text-secondary);
  margin-top: 8px;
}
</style>
