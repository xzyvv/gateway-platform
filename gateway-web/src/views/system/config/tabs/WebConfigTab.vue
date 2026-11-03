<template>
  <div v-if="web" class="web-tab">
    <el-form :model="web" label-width="120px" class="web-form">
      <el-form-item label="监听地址">
        <el-input v-model="web.listen_addr" placeholder="0.0.0.0:8080">
          <template #prefix><el-icon><Link /></el-icon></template>
        </el-input>
        <div class="field-hint">
          默认 127.0.0.1:8080 仅本机可访问；笔记本网线直连网关时改为 0.0.0.0:8080 才能被外部访问。
        </div>
      </el-form-item>
      <!-- S5: 直连调试的硬前置条件。网关对「绑定全部网卡 + 未设置访问密码」fail-closed 拒绝启动，
           但原界面完全没有提示，用户改完监听地址后重启会直接起不来、且看不出原因。 -->
      <el-alert
        v-if="bindsAll && !hasPassword"
        type="error"
        show-icon
        :closable="false"
        title="当前监听地址绑定全部网卡，但还没有设置访问密码"
        class="web-alert"
      >
        网关会拒绝以此配置启动（安全策略 fail-closed）。请先在下方「访问密码」设置密码，再保存监听地址。
      </el-alert>
      <el-form-item label="API Token">
        <el-input v-model="web.api_token" type="password" show-password placeholder="留空则保留原值">
          <template #prefix><el-icon><Key /></el-icon></template>
          <template #append>
            <el-tag
              size="small"
              :type="web.api_token && web.api_token !== originalToken ? 'warning' : 'info'"
              effect="plain"
            >
              {{ tokenStatusText }}
            </el-tag>
          </template>
        </el-input>
      </el-form-item>
      <el-form-item label="允许匿名读">
        <el-switch v-model="web.allow_anon_read" active-text="开启" inactive-text="关闭" />
        <div class="field-hint">
          仅放宽看板类只读数据；配置、审计、密钥、健康度等敏感读接口始终要求 token。
        </div>
      </el-form-item>
      <el-form-item label="Webhook 测试白名单">
        <el-input
          :model-value="allowHostsText"
          placeholder="留空 = 不限制，如 10.1.2.3, erp.local:8080"
          @update:model-value="onAllowHostsInput"
        />
        <div class="field-hint">
          逗号分隔的 host 或 host:port；填写后「webhook 连通性测试」只允许访问清单内的目标。
        </div>
      </el-form-item>
      <el-form-item label="禁止测试内网">
        <el-switch v-model="web.webhook_test_block_private" active-text="开启" inactive-text="关闭" />
        <div class="field-hint">
          开启后，webhook 测试禁止访问回环/私网/CGNAT/链路本地地址（严格模式不跟随重定向）。
        </div>
      </el-form-item>
    </el-form>

    <!-- 访问密码：独立走 /api/auth/password（服务端哈希），不写 password_hash 字段 -->
    <div class="pwd-block">
      <div class="pwd-head">
        <span class="pwd-title">访问密码</span>
        <el-tag size="small" :type="hasPassword ? 'success' : 'warning'" effect="plain">
          {{ hasPassword ? '已设置' : '仍为默认密码 admin / admin' }}
        </el-tag>
      </div>
      <div class="field-hint pwd-hint">
        密码由网关加盐哈希后写入配置，此处只提交新旧密码，不直接编辑哈希值。
        未设置密码时，当前密码填 admin 即可。
      </div>
      <el-form label-width="120px" class="pwd-form" @submit.prevent>
        <el-form-item label="当前密码">
          <el-input v-model="oldPassword" type="password" show-password autocomplete="current-password" placeholder="默认密码为 admin" />
        </el-form-item>
        <el-form-item label="新密码">
          <el-input v-model="newPassword" type="password" show-password autocomplete="new-password" placeholder="至少 6 位" />
        </el-form-item>
        <el-form-item label="确认新密码">
          <el-input v-model="confirmPassword" type="password" show-password autocomplete="new-password" placeholder="再输入一次" />
        </el-form-item>
        <el-form-item>
          <el-button type="primary" :loading="savingPwd" :disabled="!canSubmitPwd" @click="submitPassword">
            修改密码
          </el-button>
        </el-form-item>
      </el-form>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { ElMessage } from 'element-plus'
import type { WebConfig } from '@/api/types'
import { changePassword } from '@/api/auth'

const web = defineModel<WebConfig | null>({ required: true })
const props = defineProps<{ originalToken: string; tokenStatusText: string }>()

/** password_hash 在 GET /api/config 中被脱敏为 `********`；空串表示未设置（仍用默认密码） */
const hasPassword = computed(() => !!web.value?.password_hash)
const bindsAll = computed(() => {
  const addr = (web.value?.listen_addr || '').trim()
  return addr.startsWith('0.0.0.0') || addr.startsWith('[::]')
})

const oldPassword = ref('')
const newPassword = ref('')
const confirmPassword = ref('')
const savingPwd = ref(false)
const canSubmitPwd = computed(
  () => !!oldPassword.value && newPassword.value.length >= 6 && newPassword.value === confirmPassword.value,
)

/** Webhook 测试白名单：配置里是数组，表单以逗号分隔文本编辑（编辑态不丢未提交的输入） */
const allowHostsText = computed(() => (web.value?.webhook_test_allow_hosts ?? []).join(', '))
function onAllowHostsInput(v: string) {
  if (!web.value) return
  web.value.webhook_test_allow_hosts = v
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
}

async function submitPassword() {
  if (newPassword.value !== confirmPassword.value) {
    ElMessage.warning('两次输入的新密码不一致')
    return
  }
  savingPwd.value = true
  try {
    await changePassword(oldPassword.value, newPassword.value)
    // 改密成功后服务端会轮换 api_token，此处只需刷新展示态
    if (web.value) web.value.password_hash = '********'
    oldPassword.value = ''
    newPassword.value = ''
    confirmPassword.value = ''
    ElMessage.success('访问密码已更新')
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : '修改密码失败')
  } finally {
    savingPwd.value = false
  }
}
// props 仅用于类型声明，`originalToken` / `tokenStatusText` 已在模板中使用
void props
</script>

<style scoped>
.web-tab {
  display: flex;
  flex-direction: column;
  gap: 16px;
}
.web-alert {
  margin-bottom: 4px;
}
.field-hint {
  margin-top: 4px;
  font-size: 0.75rem;
  line-height: 1.5;
  color: var(--text-secondary);
}
.pwd-block {
  border-top: 1px solid var(--border-color-lighter);
  padding-top: 16px;
}
.pwd-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 4px;
}
.pwd-title {
  font-weight: 600;
  color: var(--text-primary);
}
.pwd-hint {
  margin-bottom: 12px;
}
.pwd-form {
  max-width: 480px;
}
</style>
