<template>
  <el-card shadow="never" class="section-card">
    <template #header>
      <SectionHeader :title="title" icon="Document">
        <template #extra>
          <el-tag :type="result.valid ? 'success' : 'danger'" effect="dark" round size="small">
            {{ result.valid ? '校验通过' : '校验失败' }}
          </el-tag>
          <el-tag type="info" effect="plain" round size="small">{{ result.protocol.toUpperCase() }}</el-tag>
        </template>
        <el-button size="small" link @click="copyResult">复制结果</el-button>
      </SectionHeader>
    </template>
    <div v-if="result.error" class="error-msg">
      <el-alert :title="result.error" type="error" :closable="false" show-icon />
    </div>
    <p v-if="hint" class="hint">{{ hint }}</p>
    <el-descriptions :column="1" border>
      <el-descriptions-item label="原始 HEX">
        <code class="hex">{{ result.rawHex }}</code>
      </el-descriptions-item>
      <el-descriptions-item v-for="(f, i) in result.fields" :key="i" :label="f.name">
        <span class="field-value">{{ f.value }}</span>
        <span v-if="f.desc" class="field-desc">（{{ f.desc }}）</span>
      </el-descriptions-item>
    </el-descriptions>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 帧解析结果展示（C4 内核收敛的展示侧）
 *
 * 离线样本（FrameParser）与在线真实响应（PacketEditor）都用这一个组件渲染同一份
 * `ParseResult`，避免两处各写一套字段表：过去在线结果只有后端一句 `parsed` 文案，
 * 与离线逐字段表格对不上，现场核对同一帧要在两处来回看。
 */
import SectionHeader from '@/components/SectionHeader.vue'
import { copyText } from '@/utils/clipboard'
import { parseResultToText, type ParseResult } from '@/utils/frameParser'

const props = withDefaults(defineProps<{
  result: ParseResult
  /** 区块标题：离线叫「帧解析」，在线响应叫「响应帧解析」 */
  title?: string
  /** 可选说明：解释数据来源（如「来自本次真实下发」） */
  hint?: string
}>(), {
  title: '帧解析',
})

function copyResult() {
  void copyText(parseResultToText(props.result), '解析结果已复制')
}
</script>

<style scoped>
.section-card { border-radius: var(--radius-card); }
.error-msg { margin-bottom: 12px; }
.hint { margin: 0 0 10px; font-size: 0.8125rem; color: var(--text-secondary); }
.hex {
  font-family: monospace;
  background: var(--bg-fill);
  padding: 2px 6px;
  border-radius: var(--radius-small);
}
.field-value { font-weight: 500; }
.field-desc { color: var(--text-secondary); font-size: 0.75rem; margin-left: 6px; }
</style>
