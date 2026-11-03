<template>
  <div class="uplink-config">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="上送模板" icon="Upload">
          <template #extra>
            <el-tag v-if="cfg.enabled" type="success" effect="dark" round size="small">已启用</el-tag>
            <el-tag v-else type="info" effect="plain" round size="small">未启用</el-tag>
          </template>
          <el-button size="small" @click="loadConfig" :loading="loading"><el-icon><Refresh /></el-icon> 重新加载</el-button>
          <el-button size="small" @click="doExportTemplate" :loading="exporting"><el-icon><Download /></el-icon> 导出模板</el-button>
          <el-button size="small" @click="triggerImport"><el-icon><Upload /></el-icon> 导入模板</el-button>
          <input ref="importInput" type="file" accept=".json,.txt" style="display:none" @change="handleImport" />
          <el-button size="small" @click="goTester">
            <el-icon><Promotion /></el-icon> 预览渲染 / 测试上行
          </el-button>
          <el-button type="primary" size="small" :loading="saving" @click="saveConfig" :disabled="!cfg.enabled">
            <el-icon><Check /></el-icon> 保存配置
          </el-button>
        </SectionHeader>
      </template>

      <!-- 2.4s 超时兜底：网关请求超过 2.4s 仍未返回时提示用户（loading=false 自动消失） -->
      <el-alert
        v-if="slowLoading"
        type="info"
        :closable="false"
        show-icon
        class="form-slow-hint"
      >
        <template #title>网关未及时响应，继续等待或稍后重试</template>
      </el-alert>

      <el-form label-width="100px">
        <el-form-item label="启用模板">
          <el-switch v-model="cfg.enabled" active-text="自定义模板" inactive-text="内置默认" />
          <span style="margin-left: 12px; color: var(--text-secondary); font-size: 0.75rem">
            关闭时使用网关默认 JSON 格式（按设备分组嵌套），不影响 MQTT 上行
          </span>
        </el-form-item>

        <el-form-item label="编码格式">
          <el-select v-model="cfg.encoding" style="width: 200px" :disabled="!cfg.enabled">
            <el-option label="JSON（单条/嵌套）" value="json" />
            <el-option label="JSON Lines（每行一条）" value="json_lines" />
            <el-option label="纯文本（自定义）" value="text" />
          </el-select>
          <span style="margin-left: 12px; color: var(--text-secondary); font-size: 0.75rem">
            服务器期望的格式（ThingsBoard/阿里云用 JSON；InfluxDB 用纯文本）
          </span>
        </el-form-item>

        <el-form-item label="单条字节上限">
          <el-input-number v-model="cfg.max_bytes" :min="64" :max="65536" :step="64" :disabled="!cfg.enabled" />
          <span style="margin-left: 12px; color: var(--text-secondary); font-size: 0.75rem">
            渲染后超过此大小的报文将被丢弃（保护 MQTT broker）
          </span>
        </el-form-item>

        <el-form-item label="模板内容">
          <div class="template-editor">
            <el-input
              v-model="cfg.template"
              type="textarea"
              :rows="14"
              spellcheck="false"
              :disabled="!cfg.enabled"
              class="template-textarea"
              placeholder='{"ts":${timestamp_ms},"device":"${device_id}","value":${value}}'
            />
            <div class="editor-footer">
              <span style="color: var(--text-secondary); font-size: 0.75rem">
                字节数：{{ templateBytes }} | 变量数：{{ variableCount }} | 状态：
                <el-tag v-if="compileError" type="danger" effect="dark" size="small">编译失败</el-tag>
                <el-tag v-else type="success" effect="dark" size="small">就绪</el-tag>
              </span>
              <span v-if="compileError" class="compile-error">{{ compileError }}</span>
            </div>
          </div>
        </el-form-item>

        <el-form-item label="模板库">
          <el-select v-model="selectedTemplateId" placeholder="选择预设模板快速填充" style="width: 280px" clearable @change="applyTemplate">
            <el-option v-for="tpl in BUILTIN_TEMPLATES" :key="tpl.id" :label="tpl.name" :value="tpl.id">
              <span style="float: left">{{ tpl.name }}</span>
              <span style="float: right; color: var(--text-secondary); font-size: 0.75rem; margin-left: 8px">{{ tpl.description }}</span>
            </el-option>
          </el-select>
          <el-button size="small" @click="goTester" style="margin-left: 8px">
            <el-icon><Promotion /></el-icon> 前往调试工具
          </el-button>
        </el-form-item>

        <el-form-item label="可用变量">
          <el-collapse v-model="varsPanelOpen">
            <el-collapse-item name="vars" :title="`点击展开 / 收起变量清单（${totalVariables} 个）`">
              <DataTable
                :data="templateVars"
                :columns="varColumns"
                row-key="name"
                stripe
              />
              <div class="vars-hint">
                <el-alert type="info" :closable="false" show-icon>
                  <template #title>
                    <strong>使用提示</strong>
                  </template>
                  <div>• 变量输出 <strong>裸值</strong>，不带引号/转义。字符串场景需手写 <code>"${'${var}'}"</code></div>
                  <div>• 同一变量可作 JSON key（如 ThingsBoard）或 JSON value</div>
                  <div>• 数字 NaN/Inf 自动输出 <code>null</code>，保证 JSON 合法</div>
                  <div>• 未知变量静默渲染为空字符串，不阻断上行</div>
                </el-alert>
              </div>
            </el-collapse-item>
          </el-collapse>
        </el-form-item>

        <el-form-item v-if="cfg.updated_at" label="最后更新">
          <el-tag type="info" effect="plain" round size="small">
            <el-icon><Clock /></el-icon> {{ cfg.updated_at }}
          </el-tag>
        </el-form-item>
      </el-form>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { h, ref, watch, onUnmounted } from 'vue'
import { ElButton, ElTag } from 'element-plus'
import { Plus } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import type { TemplateVarInfo } from '@/api/uplink'
import { useUplinkConfig } from '@/composables/uplink/useUplinkConfig'

const {
  loading, saving, exporting,
  templateVars, varsPanelOpen, selectedTemplateId, cfg,
  compileError, templateBytes, variableCount, totalVariables,
  BUILTIN_TEMPLATES,
  loadConfig, saveConfig,
  doExportTemplate, triggerImport, handleImport, importInput,
  applyTemplate, insertVar, goTester,
} = useUplinkConfig()

// 模板里的 ref="importInput" 是按名字绑定 setup 变量的，vue-tsc 的 noUnusedLocals 看不到这层引用
void importInput

// 变量表统一走 DataTable；列顺序/宽度与迁移前保持一致
const varColumns: ColumnDef<TemplateVarInfo>[] = [
  {
    key: 'name',
    title: '变量名',
    width: 160,
    cell: (row) => h('code', { class: 'var-name' }, `\${${row.name}}`),
  },
  {
    key: 'type',
    title: '类型',
    width: 100,
    cell: (row) =>
      h(
        ElTag,
        {
          size: 'small',
          type: row.type === 'number' ? 'primary' : row.type === 'bool' ? 'warning' : 'info',
          effect: 'plain',
          round: true,
        },
        () => row.type,
      ),
  },
  { key: 'description', title: '说明' },
  {
    key: 'actions',
    title: '示例值',
    width: 120,
    cell: (row) =>
      h(
        ElButton,
        { text: true, size: 'small', icon: Plus, onClick: () => insertVar(row.name) },
        () => '插入',
      ),
  },
]

// 表单请求超过 2.4s 仍未返回时，在表单顶部显示 info 提示（v-loading 已移除）。
const slowLoading = ref(false)
let slowTimer: ReturnType<typeof setTimeout> | null = null
watch(loading, (val) => {
  if (val) {
    slowLoading.value = false
    slowTimer = setTimeout(() => { slowLoading.value = true }, 2400)
  } else {
    if (slowTimer) { clearTimeout(slowTimer); slowTimer = null }
    slowLoading.value = false
  }
})
onUnmounted(() => {
  if (slowTimer) clearTimeout(slowTimer)
})
</script>

<style scoped src="./UplinkConfig.css"></style>
