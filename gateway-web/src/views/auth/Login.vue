<template>
  <div class="login-container">
    <div class="login-bg-shape shape-1"></div>
    <div class="login-bg-shape shape-2"></div>
    <div class="login-bg-shape shape-3"></div>
    <el-card class="login-card">
      <div class="login-logo">
        <!-- 与侧栏共用同一份品牌标识（BrandMark），登录前后造型一致 -->
        <BrandMark :size="44" class="login-logo-mark" />
      </div>
      <div class="login-header">
        <h1>工业网关</h1>
        <p>直连调试控制台</p>
      </div>

      <!-- 浏览器模式：页面由当前地址提供，接口与 WebSocket 同源，无需填写网关地址。
           右侧状态不是静态文案：挂载时真实探测 /health/ready，连不上就明确说「无响应」。 -->
      <div v-if="!isDesktop" class="direct-gateway" :title="'页面由 ' + currentHost + ' 提供，登录接口与实时推送均走同源，无需填写网关地址'">
        <el-icon class="direct-gateway-icon"><Connection /></el-icon>
        <span class="direct-gateway-host">{{ currentHost }}</span>
        <el-tag size="small" :type="probeTagType" effect="plain" round>{{ probeText }}</el-tag>
      </div>

      <template v-if="isDesktop">
        <!-- 桌面端：局域网自动发现的网关候选 -->
        <div v-if="discovery.found.value.length > 0 || discovery.scanning.value" class="discovered-list">
        <div class="discovered-header">
          <el-icon><Cellphone /></el-icon>
          <span>局域网已发现网关</span>
          <el-button
            v-if="!discovery.scanning.value"
            link
            size="small"
            type="primary"
            @click="rescan"
          >重新扫描</el-button>
          <el-button
            v-else
            link
            size="small"
            type="danger"
            @click="cancelScan"
          >取消</el-button>
        </div>
        <div v-if="discovery.scanning.value && discovery.found.value.length === 0" class="discovered-scanning">
          <el-icon class="is-loading"><Loading /></el-icon>
          <span>正在扫描同网段网关（最多 8 秒）…</span>
        </div>
        <ul v-else class="discovered-items">
          <li v-for="gw in discovery.found.value" :key="gw.url">
            <!-- 用 button 而非 li+@click：键盘用户可 Tab 聚焦、Enter 选择 -->
            <button
              type="button"
              class="discovered-item"
              :class="{ active: gatewayUrl === gw.url }"
              :aria-pressed="gatewayUrl === gw.url"
              @click="selectDiscovered(gw.url)"
            >
              <el-icon class="item-icon"><Connection /></el-icon>
              <div class="item-info">
                <div class="item-url">{{ gw.url }}</div>
                <div class="item-meta">延迟 {{ gw.latencyMs }} ms</div>
              </div>
              <el-icon v-if="gatewayUrl === gw.url" class="item-check"><CircleCheckFilled /></el-icon>
            </button>
          </li>
        </ul>
      </div>

      <!-- 候选为空 + 未在扫描 → 提供手动扫描入口 -->
      <div v-if="!discovery.scanning.value && discovery.found.value.length === 0" class="discover-cta">
        <el-button link size="small" @click="rescan">
          <el-icon><Search /></el-icon>
          <span>扫描局域网网关</span>
        </el-button>
      </div>
      </template>

      <el-form
        :model="form"
        status-icon
        @submit.prevent="handleLogin"
      >
        <!-- 仅桌面端需要填写网关地址；浏览器直连模式走同源，只留用户名密码 -->
        <el-form-item v-if="isDesktop">
          <div class="gateway-label">
            <span>网关地址</span>
            <el-tag size="small" type="info" effect="plain" round>直连调试</el-tag>
          </div>
          <el-input
            v-model="gatewayUrl"
            aria-label="网关地址"
            placeholder="http://192.168.1.100:8080（自动扫描失败时手填）"
            size="large"
            clearable
            class="gateway-input"
          >
            <template #prefix><el-icon :class="['gateway-icon', { active: !!gatewayUrl }]"><Connection /></el-icon></template>
          </el-input>
        </el-form-item>
        <el-form-item>
          <el-input
            v-model="form.username"
            aria-label="用户名"
            placeholder="请输入用户名"
            size="large"
            autocomplete="username"
            @keyup.enter="handleLogin"
          >
            <template #prefix><el-icon><User /></el-icon></template>
          </el-input>
        </el-form-item>
        <el-form-item>
          <el-input
            v-model="form.password"
            aria-label="密码"
            type="password"
            placeholder="请输入密码"
            size="large"
            autocomplete="current-password"
            show-password
            @keyup.enter="handleLogin"
          >
            <template #prefix><el-icon><Lock /></el-icon></template>
          </el-input>
        </el-form-item>
        <el-button
          type="primary"
          size="large"
          class="login-btn"
          :loading="loading"
          @click="handleLogin"
        >
          <span>登录</span>
        </el-button>
      </el-form>
      <div class="login-tip">
        <el-icon><InfoFilled /></el-icon>
        <span>首次登录请使用初始密码，登录后及时修改以保障安全</span>
      </div>
      <div class="login-tip">
        <el-icon><InfoFilled /></el-icon>
        <span>本网关为单一管理员账号登录，不区分角色；第三方集成请使用 API 密钥（按 scope 限权）</span>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { reactive, ref, onMounted, onUnmounted, computed } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import {
  User, Lock, InfoFilled, Connection,
  Cellphone, Loading, CircleCheckFilled, Search,
} from '@element-plus/icons-vue'
import BrandMark from '@/components/BrandMark.vue'
import { useAuthStore } from '@/stores/auth'
import { getStatus } from '@/api/status'
import { isTauri, getGatewayBaseUrl, loadGatewayConfig, saveGatewayConfig } from '@/api'
import { useGatewayDiscovery } from '@/composables/system/useGatewayDiscovery'
import { scopedSet } from '@/utils/gatewayStorage'
import { statusTagType, type TagType } from '@/utils/statusType'

const router = useRouter()
const auth = useAuthStore()
const loading = ref(false)
const form = reactive({ username: '', password: '' })
const gatewayUrl = ref('')

/**
 * 运行形态：
 * - 桌面端（Tauri）：可填/切换多个网关地址，需要局域网发现
 * - 浏览器：页面由网关自身托管（或 vite 同源代理），API 与 WS 都走同源，
 *   只输入用户名密码即可调试 —— 这正是「网线直连、浏览器开 IP」的使用场景。
 */
const isDesktop = isTauri()
const currentHost = window.location.host

/**
 * 浏览器模式连通性探测
 * ─────────────────────────────
 * 「已直连」不能是一句静态文案，否则网关没起来时也在骗人。
 * 这里在挂载时真实请求 /health/ready（无需鉴权的就绪探针，503 表示在线但组件降级），
 * 用原生 fetch 而非 axios：避免探测失败时被全局拦截器弹「网络错误」打扰登录页。
 */
type ProbeState = 'checking' | 'ok' | 'fail'
const probeState = ref<ProbeState>('checking')
const probeText = computed(() => {
  if (probeState.value === 'checking') return '检测中…'
  return probeState.value === 'ok' ? '网关已连接' : '网关无响应'
})
/** 探测结果 → 徽章颜色：ok/fail 已登记在全站唯一映射表 utils/statusType，checking 落默认 info */
const probeTagType = computed<TagType>(() => statusTagType(probeState.value))

async function probeGateway() {
  probeState.value = 'checking'
  const ctrl = new AbortController()
  const timer = setTimeout(() => ctrl.abort(), 4000)
  try {
    const resp = await fetch('/health/ready', { signal: ctrl.signal, cache: 'no-store' })
    // 能拿到响应（含 503 降级）就说明同源直连成立
    probeState.value = resp.ok || resp.status === 503 ? 'ok' : 'fail'
  } catch {
    probeState.value = 'fail'
  } finally {
    clearTimeout(timer)
  }
}

// 局域网自动发现：进页面自动扫一次；用户可手动重扫 / 取消
const discovery = useGatewayDiscovery()

onMounted(async () => {
  // 网关地址 / 局域网发现只在桌面端有意义：浏览器直连模式下页面已由网关托管，
  // 再扫一遍同网段既无意义又会在控制台留下无谓的网络错误。
  if (!isDesktop) {
    // 浏览器模式：探测一次真实连通性，决定徽标显示「网关已连接 / 无响应」
    void probeGateway()
    return
  }
  // Tauri 桌面端：先读已保存的网关地址（之前手填的或上次的）
  await loadGatewayConfig()
  gatewayUrl.value = getGatewayBaseUrl()
  // 直连调试场景下自动启动局域网扫描
  // - 不阻塞其他初始化
  // - 用户不需要等结果，可以直接手填
  discovery.scan().catch(() => { /* 静默：失败也只是没候选 */ })
})

onUnmounted(() => {
  // 卸载时停掉扫描，避免组件已销毁还在 ping
  discovery.cancel()
})

async function rescan() {
  const result = await discovery.scan()
  if (result.length === 0) {
    ElMessage.info('未在局域网内发现网关，请确认网线连接或手动填写地址')
  } else if (result.length === 1) {
    // 只发现一台时自动选上，省一步操作
    const first = result[0]
    if (first) gatewayUrl.value = first.url
  }
}

function cancelScan() {
  discovery.cancel()
}

function selectDiscovered(url: string) {
  gatewayUrl.value = url
}

async function handleLogin() {
  // 回车键 + 表单 submit + 按钮 click 三个入口都可能触发：加载中直接忽略，避免重复登录
  if (loading.value) return
  if (!form.username.trim() || !form.password) {
    ElMessage.warning('请输入用户名和密码')
    return
  }
  // 桌面端：保存网关地址
  if (isTauri()) {
    const url = gatewayUrl.value.trim()
    if (!url) {
      ElMessage.warning('请输入网关地址')
      return
    }
    // 提取 host 作为默认名称（避免列表里全是 url），方便用户区分多个网关
    let name = url
    try {
      const u = new URL(url)
      name = u.hostname
    } catch { /* 保留原值 */ }
    try {
      await saveGatewayConfig({
        gateways: [{ name, url }],
        active: name,
      })
    } catch (e) {
      ElMessage.error(`保存网关地址失败: ${e}`)
      return
    }
  }
  loading.value = true
  try {
    await auth.login(form.username.trim(), form.password)
    try { scopedSet('gw_username', form.username.trim()) } catch { /* 存储不可用不阻断登录 */ }
    // getStatus 仅用于预热连接，失败不应阻断登录流程
    try { await getStatus() } catch { /* ignore */ }
    ElMessage.success('登录成功')
    router.push('/overview')
  } catch (e) {
    // W6: 在开发/调试场景展示真实错误（含网络错误、401、403、500 等），
    // 生产场景运维可借此快速定位（凭据错 vs 网络不通 vs 服务异常）。
    const detail = (e instanceof Error && e.message) ? e.message : String(e ?? '')
    ElMessage.error(detail ? `登录失败：${detail}` : '用户名或密码错误')
  } finally {
    loading.value = false
  }
}
</script>

<style scoped src="./Login.css"></style>
