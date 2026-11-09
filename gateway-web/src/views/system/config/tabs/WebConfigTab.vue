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

      <!-- 性能与限流：多数部署用默认值，折叠避免与必填项抢注意力 -->
      <el-collapse v-model="advancedOpen" class="web-advanced">
        <el-collapse-item name="advanced">
          <template #title>
            <span class="advanced-name">高级（性能与限流）</span>
          </template>
          <el-form-item label="实时推送并发上限">
            <el-input-number v-model="web.realtime_max_subscribers" :min="1" :max="1024" />
            <div class="field-hint">
              WebSocket 同时在线连接上限（1~1024，默认 64）；超限的新连接直接返回 503。保存后对新连接立即生效。
            </div>
          </el-form-item>
          <el-form-item label="实时推送缓冲">
            <PresetNumberSelect
              v-model="web.realtime_buffer"
              :options="REALTIME_BUFFER_PRESETS"
              :min="16"
              :max="65536"
              unit="条"
              aria-label="实时推送缓冲"
            />
            <div class="field-hint">
              WebSocket 断线重连时可回放的事件条数（默认 1024）；越大抗抖动越好、内存占用越高。改动需重启网关后生效。
            </div>
          </el-form-item>
          <el-form-item label="接口限流（次/秒）">
            <div class="rate-limit-field">
              <el-switch v-model="rateLimitEnabled" active-text="开启" inactive-text="关闭" />
              <PresetNumberSelect
                v-if="rateLimitEnabled"
                v-model="rateLimitValue"
                :options="RATE_LIMIT_PRESETS"
                :min="1"
                :max="10000"
                unit="次/秒"
                aria-label="接口限流"
              />
            </div>
            <div class="field-hint">
              单客户端每秒请求上限；关闭=不限流（默认）。改动需重启网关后生效。
            </div>
          </el-form-item>
          <el-form-item label="跨域来源白名单">
            <div class="cors-field">
              <el-switch v-model="corsRestricted" active-text="限制来源" inactive-text="允许任意来源" />
              <el-input
                v-if="corsRestricted"
                :model-value="corsText"
                type="textarea"
                :rows="3"
                placeholder="每行一个来源，如 http://192.168.1.10:5173"
                aria-label="跨域来源白名单"
                @update:model-value="onCorsInput"
              />
            </div>
            <div class="field-hint">
              关闭=允许任意网站跨域（默认，存在 CSRF 风险）；开启且留空=仅同源；填写=按白名单放行。改动需重启网关后生效。
            </div>
          </el-form-item>
        </el-collapse-item>
      </el-collapse>
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
import PresetNumberSelect from '@/components/industrial/PresetNumberSelect.vue'
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
/** 高级分组默认收起：性能与限流参数多数部署用默认值即可 */
const advancedOpen = ref<string[]>([])

/**
 * 接口限流开关：配置里 `null` 表示不限流。
 * 关闭时写回 null；重新开启时回落到上次的数值（默认 60 次/秒），
 * 避免「关一下再开」把现场已调好的阈值清成默认值。
 */
const LAST_RATE_LIMIT = 60
/** 常用限流档位（次/秒），仍可输入 1~10000 内任意整数 */
const RATE_LIMIT_PRESETS = [10, 30, 60, 120, 300, 600, 1000]
/** 常用回放缓冲档位（条），默认 1024 */
const REALTIME_BUFFER_PRESETS = [128, 256, 512, 1024, 2048, 4096, 8192]
const rateLimitEnabled = computed<boolean>({
  get: () => web.value?.rate_limit_per_sec != null,
  set: (on) => {
    if (!web.value) return
    web.value.rate_limit_per_sec = on ? LAST_RATE_LIMIT : null
  },
})
const rateLimitValue = computed<number | null>({
  get: () => web.value?.rate_limit_per_sec ?? LAST_RATE_LIMIT,
  set: (v) => {
    // 下拉只会回传数字；null 保持原值不写入，避免非法配置
    if (!web.value || v == null) return
    web.value.rate_limit_per_sec = v
  },
})

/**
 * 跨域来源白名单（后端三态）：
 * - `null` = 不限制，允许任意来源（默认，校验会告警）；
 * - `[]` = 仅同源；
 * - 非空数组 = 白名单。
 * 关闭开关写回 null；开启先落到 []，避免「开了却仍然全放开」的错觉。
 */
const corsRestricted = computed<boolean>({
  get: () => web.value?.cors_origins != null,
  set: (on) => {
    if (!web.value) return
    web.value.cors_origins = on ? [] : null
  },
})

const corsText = computed(() => (web.value?.cors_origins ?? []).join('\n'))

function onCorsInput(v: string) {
  if (!web.value) return
  // 逗号与换行都当分隔符，逐项 trim；清空即 []（仅同源）
  web.value.cors_origins = v
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean)
}
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
.web-advanced {
  margin-top: 4px;
}
.web-advanced :deep(.el-collapse-item__header),
.web-advanced :deep(.el-collapse-item__wrap) {
  padding-left: 0;
  padding-right: 0;
}
.advanced-name {
  font-weight: 600;
}
.rate-limit-field {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
}
.cors-field {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 100%;
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
