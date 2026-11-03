import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { ElMessage } from 'element-plus'
// EP 官方暗色变量（挂在 html.dark 下，与 theme store 切换的 class 配套）
import 'element-plus/theme-chalk/dark/css-vars.css'
import '../assets/styles/global.css'
// 首屏只注册登录页需要的 5 个图标，其余 77 个在挂载后懒加载注册（utils/icons-extra.ts），
// 避免登录页白屏时加载 120KB 图标。
import { Cellphone, Search, User, Lock, InfoFilled } from '@element-plus/icons-vue'

// 登录页图标（首屏必需）
const coreIcons = { Cellphone, Search, User, Lock, InfoFilled }

import App from './App.vue'
import router from '../router'
import { isTauri, loadGatewayConfig, checkGatewayHealth } from '@/api'
import SectionHeader from '../components/SectionHeader.vue'
import EmptyState from '../components/EmptyState.vue'
import { useThemeStore } from '../stores/theme'
import { useAuthStore } from '../stores/auth'
import { logWarn, logError } from '../utils/logger'

const app = createApp(App)

// 注册 Element Plus 图标（首屏仅注册登录页所需 5 个）
for (const [key, component] of Object.entries(coreIcons)) {
  app.component(key, component)
}

// 注册全局公共组件
app.component('SectionHeader', SectionHeader)
app.component('EmptyState', EmptyState)

// 全局错误处理：弹窗提示用户，避免页面静默失败
app.config.errorHandler = (err, _instance, info) => {
  logError('main', err, '[全局错误]', info)
  ElMessage({
    message: '页面发生错误，请刷新或联系管理员',
    type: 'error',
    duration: 5000,
    showClose: true,
    grouping: true,
  })
}

// 避免 ElMessage 自身报错时无限递归
const _origErrHandler = app.config.errorHandler
app.config.errorHandler = (err, instance, info) => {
  if (info && String(info).startsWith('ElMessage')) {
    logError('main', err, '[ElMessage 内部错误]')
    return
  }
  return _origErrHandler ? _origErrHandler(err, instance, info) : undefined
}

// 未处理的 Promise 拒绝：console + 弹窗
// 用最近一次错误时间戳去重，避免短时间内同一异常刷屏
let _lastUnhandledAt = 0
let _lastUnhandledMsg = ''
window.addEventListener('unhandledrejection', (event) => {
  const reason = event.reason
  logError('main', reason, '[未处理的 Promise 拒绝]')
  const msg = reason instanceof Error ? reason.message : String(reason || 'unknown')
  const now = Date.now()
  // 同消息 3 秒内只弹一次
  if (msg === _lastUnhandledMsg && now - _lastUnhandledAt < 3000) return
  _lastUnhandledAt = now
  _lastUnhandledMsg = msg
  ElMessage({
    message: '异步操作失败: ' + msg,
    type: 'error',
    duration: 4000,
    showClose: true,
  })
})

// 动态页面标题
router.afterEach((to) => {
  const title = (to.meta.title as string) || '工业网关'
  document.title = `${title} - 工业网关调试控制台`
})

const pinia = createPinia()
app.use(pinia)
app.use(router)

// 挂载前恢复上次主题偏好（localStorage），避免暗色用户首屏闪白
useThemeStore(pinia).initTheme()

// 桌面端：启动时加载网关配置（激活的网关地址），确保首次 API 调用前已就绪
// 启动性能：配置加载与 token 迁移是本地 IO，并行执行；健康检查是网络请求
//（网关不可达时要等到超时），移出首屏等待，避免桌面端启动白屏数秒
async function bootstrap() {
  try {
    if (isTauri()) {
      // token 迁移与配置加载并行：迁移失败只降级明文，不影响挂载；
      // 未迁移完成的首个请求会临时读到明文 token（同一 localStorage，无损坏风险）
      const migratePromise = import('@/api/token')
        .then(({ migrateLegacyToken }) => migrateLegacyToken())
        .then(async (migrated) => {
          if (migrated) {
            logWarn('bootstrap', 'legacy plaintext token migrated to encrypted storage')
          }
          // Bug-W7 (fix)：token 迁移完成后，读取 secureStorage 的降级状态——
          // 若 IndexedDB / Web Crypto 在隐私模式或受限环境下不可用，token 已静默
          // 落明文 localStorage，必须显式告知用户「凭证未加密保存」。
          // 这里在挂载前读，确保 ElMessage 在 pinia + ElementPlus 就绪后能正常渲染。
          const { __getLastDegradeReason } = await import('../utils/secureStorage')
          const reason = __getLastDegradeReason()
          if (reason) {
            // 延迟到挂载后再弹（app.mount 之后 ElMessage 才能拿到完整主题/全局样式）
            queueMicrotask(() => {
              ElMessage({
                message:
                  '当前浏览器禁用了加密存储（隐私模式或受限环境），' +
                  'API Token 将以明文保存。建议关闭隐私窗口或更换浏览器。',
                type: 'warning',
                duration: 8000,
                showClose: true,
                grouping: true,
              })
            })
            logWarn('bootstrap', 'secureStorage degraded to plaintext, reason:', reason)
          }
        })
        .catch((e) => {
          logWarn('bootstrap', 'token migration failed (will fall back to plaintext):', e)
        })

      // 配置文件缺失 / 反序列化失败时，loadGatewayConfig 内部已经返回空配置兜底
      await Promise.all([loadGatewayConfig(), migratePromise])
    }
  } catch (e) {
    // 网关配置加载失败兜底：仍然挂载应用，让 Login.vue 引导用户重新填写地址
    logError('main', e, '[bootstrap] loadGatewayConfig 失败')
  }

  app.mount('#app')

  // 懒加载并注册其余 77 个图标：登录页只需要 5 个，其余在用户登录后才用到。
  // 放在 mount 之后、用户交互之前完成（登录流程的网络请求足以覆盖导入耗时）。
  void import('../utils/icons-extra')
    .then(({ registerExtraIcons }) => registerExtraIcons(app))
    .catch((e) => logError('main', e, '[bootstrap] 图标懒加载注册失败'))

  // Bug 2.8: 启动时后台验证 token 是否有效，避免 UI 在 whoami 未返回前显示"已登录"
  // 仅在 localStorage 中存在 token 时才调：不在则不走网络请求，加速首屏
  // void 抛出后续 then/catch，不阻塞后续逻辑
  void (async () => {
    const auth = useAuthStore(pinia)
    if (auth.loggedIn) {
      await auth.validate().catch(() => { /* 主流程在 router.beforeEach 里也调，此处后台调为首屏提前刷新 */ })
    }
  })()

  // opt-6: 直连启动检测——挂载后后台执行，激活网关不可达时 warn，让用户
  // 在登录前就发现问题。不弹窗（避免启动闪窗打扰），health badge 会继续轮询展示实时状态。
  if (isTauri()) {
    void checkGatewayHealth()
      .then((health) => {
        if (health && !health.reachable) {
          logWarn('bootstrap', '激活网关不可达:', health.error)
        }
      })
      .catch(() => { /* 健康检查失败不影响已挂载的应用 */ })
  }
}

bootstrap().catch((e) => {
  // 极小概率（Vue 自身挂载失败）：打印并暴露给全局错误处理
  logError('main', e, '[bootstrap] 致命错误')
})
