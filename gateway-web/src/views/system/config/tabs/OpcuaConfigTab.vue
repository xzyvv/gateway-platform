<template>
  <div class="section-toggle">
    <el-switch
      :model-value="opcua !== null"
      @update:model-value="(val: boolean) => (opcua = val ? createDefaultOpcua() : null)"
      active-text="启用 OPC UA Server"
      inactive-text="禁用"
    />
  </div>
  <el-form v-if="opcua" :model="opcua" label-width="180px" class="opcua-form">
    <el-form-item label="启用对外服务">
      <el-switch v-model="form.enabled" />
    </el-form-item>
    <el-form-item label="监听端点">
      <el-input v-model="form.endpoint" placeholder="opc.tcp://0.0.0.0:4840">
        <template #prefix><el-icon><Share /></el-icon></template>
      </el-input>
      <div class="form-hint">格式 opc.tcp://host:port，默认端口 4840。</div>
    </el-form-item>
    <el-form-item label="允许匿名/明文">
      <el-switch v-model="form.allow_anonymous" />
      <div class="form-hint">
        配置证书后默认只暴露加密端点；仅在本地调试时打开。未配置任何用户时仍保留匿名登录。
      </div>
    </el-form-item>

    <!-- 证书与客户端信任属于一次性配置项，默认折叠；标题摘要直接显示当前取值 -->
    <el-collapse v-model="advancedOpen" class="opcua-advanced">
      <el-collapse-item name="security">
        <template #title>
          <div class="advanced-title">
            <span class="advanced-name">证书与客户端信任</span>
            <span class="advanced-summary">{{ securitySummary }}</span>
          </div>
        </template>
        <el-form-item label="服务端证书">
          <el-input v-model="certPathProxy" clearable placeholder="留空=沿用全局设备证书或临时自签名证书" />
          <div class="form-hint">PEM 证书路径，必须与私钥成对配置；文件缺失时服务拒绝启动。</div>
        </el-form-item>
        <el-form-item label="服务端私钥">
          <el-input v-model="keyPathProxy" clearable placeholder="留空=同上" />
        </el-form-item>
        <el-form-item label="信任未登记客户端证书">
          <el-switch v-model="form.trust_client_certs" />
          <div class="form-hint">
            生产环境建议关闭：关闭后未受信的客户端在加密握手阶段即被拒绝，需在 PKI 目录登记。
          </div>
        </el-form-item>
      </el-collapse-item>
    </el-collapse>

    <el-form-item label="用户名/密码用户">
      <div class="user-list">
        <div v-if="form.users.length === 0" class="form-hint">未配置用户：保留匿名登录（兼容存量客户端）。</div>
        <div v-for="(u, idx) in form.users" :key="idx" class="user-row">
          <el-input v-model="u.username" placeholder="用户名" />
          <el-input v-model="u.password" type="password" show-password placeholder="密码" />
          <el-button text type="danger" @click="removeUser(idx)">删除</el-button>
        </div>
        <el-button size="small" plain @click="addUser">添加用户</el-button>
        <div class="form-hint">
          用户名同时作为 user token id，须非空且唯一。密码保存后仅以 ******** 回显，保持原样即沿用已保存密码。
        </div>
      </div>
    </el-form-item>

    <el-form-item label="允许客户端写回点位">
      <el-switch v-model="form.write_enabled" />
      <div class="form-hint">
        开启后仅点位配置中 writable=true 的节点可被写入，写入会下发到南向采集器并记录审计。
      </div>
    </el-form-item>
  </el-form>
</template>

<script setup lang="ts">
/** OPC UA Server（对外服务）配置页签（F-14 / F-2）。
 *  后端 north/server/opcua_server.rs 已接入 bootstrap，支持用户名 token、证书覆盖、
 *  客户端证书信任策略与点位写回；本页负责这些配置节的可视化编辑。
 *  null 表示「不配置该节」，与后端 Option<OpcuaConfig> 对齐。 */
import { computed, ref } from 'vue'
import { Share } from '@element-plus/icons-vue'
import type { OpcuaConfig } from '@/api/types'

const opcua = defineModel<OpcuaConfig | null>({ required: true })
/** 表单渲染只在 opcua 非空时发生；此视图仅用于让模板拿到非空类型 */
const form = computed(() => opcua.value as OpcuaConfig)

/** 高级分组（证书/信任）默认收起：多数部署沿用全局证书与默认信任策略 */
const advancedOpen = ref<string[]>([])

/** 新建配置节时补齐后端必填字段，避免生成缺字段的 JSON */
function createDefaultOpcua(): OpcuaConfig {
  return {
    enabled: true,
    endpoint: 'opc.tcp://0.0.0.0:4840',
    allow_anonymous: false,
    cert_path: null,
    key_path: null,
    trust_client_certs: true,
    users: [],
    write_enabled: false,
  }
}

/** 证书路径的空串代理：el-input 的 clearable 只会产出 ''，需要还原为后端约定的 null，
 *  否则空串会被当成已配置路径，触发「证书与私钥必须成对配置」的语义校验失败。 */
const certPathProxy = computed<string>({
  get: () => opcua.value?.cert_path ?? '',
  set: (v) => {
    if (opcua.value) opcua.value.cert_path = v === '' ? null : v
  },
})
const keyPathProxy = computed<string>({
  get: () => opcua.value?.key_path ?? '',
  set: (v) => {
    if (opcua.value) opcua.value.key_path = v === '' ? null : v
  },
})

/** 折叠状态下也能确认证书与信任策略是否已配置 */
const securitySummary = computed(() => {
  const c = opcua.value
  if (!c) return ''
  const cert = c.cert_path || c.key_path ? '自定义证书' : '默认证书'
  const trust = c.trust_client_certs ? '自动信任客户端' : '仅信任已登记客户端'
  return `${cert} · ${trust}`
})

function addUser() {
  opcua.value?.users.push({ username: '', password: '' })
}

function removeUser(idx: number) {
  opcua.value?.users.splice(idx, 1)
}
</script>

<style scoped>
.form-hint {
  margin-top: 4px;
  font-size: 0.72rem;
  color: var(--text-secondary);
  line-height: 1.4;
}
.opcua-advanced {
  margin-bottom: 18px;
}
.opcua-advanced :deep(.el-collapse-item__header),
.opcua-advanced :deep(.el-collapse-item__wrap) {
  padding-left: 0;
  padding-right: 0;
}
.advanced-title {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.advanced-name {
  font-weight: 600;
}
.advanced-summary {
  font-size: 0.75rem;
  color: var(--text-secondary);
}
.user-list {
  width: 100%;
}
.user-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
</style>
