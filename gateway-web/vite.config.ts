import { defineConfig } from 'vite'
import { readdirSync, readFileSync, existsSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath, URL } from 'node:url'
import vue from '@vitejs/plugin-vue'
import AutoImport from 'unplugin-auto-import/vite'
import Components from 'unplugin-vue-components/vite'
import { ElementPlusResolver } from 'unplugin-vue-components/resolvers'

// Vite dev 代理：
//   - 前端 http://localhost:5173
//   - 所有 /api/* 请求（REST + WebSocket）代理到 Rust 网关
//   - WS 端点 /api/realtime 与 /api/diagnostics/logs 通过 ws: true 走 upgrade
//   - /health/* 公开健康检查也代理过去（供前端健康徽章）
// 生产部署：移除 proxy，用 nginx 把 dist/ 静态文件 + /api/* 反代到 gateway.exe。
//
// S4：target 改为可配置。原实现硬编码 http://127.0.0.1:8080，而现场调试是
// 「笔记本网线直连网关」——网关 IP 不是 127.0.0.1，前端 dev server 因此无法
// 指向真实设备。现在按顺序取：
//   1. 环境变量 GATEWAY_API_TARGET（如 http://192.168.1.50:8080）
//   2. 环境变量 GATEWAY_HOST + GATEWAY_PORT（拆开写法，便于脚本透传）
//   3. 缺省 http://127.0.0.1:8080（本机联调行为不变）
const gatewayHost = process.env.GATEWAY_HOST || '127.0.0.1'
const gatewayPort = process.env.GATEWAY_PORT || '8080'
const apiTarget = process.env.GATEWAY_API_TARGET || `http://${gatewayHost}:${gatewayPort}`

/** 递归收集 src 下的业务源码（排除单测），用于推断 element-plus 组件用量 */
function collectSources(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name)
    if (statSync(full).isDirectory()) collectSources(full, out)
    else if (/\.(vue|ts)$/.test(name) && !name.includes('.spec.')) out.push(full)
  }
  return out
}

/**
 * 预打包 element-plus 的按需组件样式。
 *
 * 为什么需要：组件样式是 unplugin-vue-components 在 transform 阶段注入的
 * （`<el-input-number>` → `element-plus/es/components/input-number/style/css`），
 * 而 optimizeDeps 的扫描器只看原始源码，看不到这些 import。于是 dev server 要等
 * 某个页面真的渲染出该组件时才发现「新依赖」，触发 re-optimize → 整页 reload。
 * E2E 里这次 reload 会把正在填写的表单清空、把懒加载的 tab 打回「加载中」，
 * 表现为「登录成功却卡在登录页」「页面一直加载中」这类假红（实测 CI 必现）。
 *
 * 这里在配置加载时把源码里出现的 el-* 组件样式显式加进 include，启动即预打包完，
 * 运行期不再触发 re-optimize。用不到的（子组件目录不存在）自动跳过。
 */
function elementPlusIncludes(): string[] {
  const compDir = 'node_modules/element-plus/es/components'
  if (!existsSync(compDir)) return []
  const names = new Set<string>()
  for (const file of collectSources('src')) {
    for (const m of readFileSync(file, 'utf8').matchAll(/<el-([a-z0-9-]+)/g)) names.add(m[1])
  }
  const includes = [...names]
    .filter((n) => existsSync(join(compDir, n, 'style/css.mjs')))
    .map((n) => `element-plus/es/components/${n}/style/css`)
  // 包根（ElMessage/ElMessageBox 等函数式组件由 AutoImport 注入）与组件公共基础样式，
  // 同样只在 transform 阶段出现，扫描器看不到
  includes.push('element-plus/es', 'element-plus/es/components/base/style/css')
  return includes
}

export default defineConfig({
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      // el-icons 瘦身：把 @element-plus/icons-vue 重定向到紧凑版
      // （src/utils/ep-icons-compact.ts，自动生成）。原版每个图标独立
      // defineComponent 包装，100 个图标约 52KB raw / 13KB gzip；
      // 紧凑版用共享工厂 ic()，体积降约 40%。
      '@element-plus/icons-vue': fileURLToPath(new URL('./src/utils/ep-icons-compact.ts', import.meta.url)),
    },
  },
  optimizeDeps: {
    // 扫描全部业务源（含懒加载路由），让 dev server 启动时一次性预构建完依赖。
    // 否则运行中首次进入某路由发现新依赖会触发 "deps optimized → 整页 reload"，
    // 会打断 E2E（表单填写中途被刷新清空），也是开发时首次切页面偶发刷新的根因。
    entries: ['index.html', 'src/**/*.{ts,vue}', '!**/*.spec.ts'],
    // 扫描器看不到按需样式（见 elementPlusIncludes 注释），显式补齐
    include: elementPlusIncludes(),
  },
  plugins: [
    vue(),
    // Element Plus 按需引入：自动导入组件与样式，避免全量打包
    AutoImport({ resolvers: [ElementPlusResolver()] }),
    Components({ resolvers: [ElementPlusResolver()] }),
  ],
  server: {
    host: '0.0.0.0',
    port: 5173,
    proxy: {
      '/api': {
        target: apiTarget,
        changeOrigin: true,
        ws: true,
      },
      // 正则形式 + 尾斜杠：只代理 /health/live、/health/ready 等探针路径，
      // 避免前缀误伤前端路由 /health-monitor、/health-score（F5 刷新会被劫持到后端兜底页）
      '^/health/': {
        target: apiTarget,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    // 阈值从 1500 收紧到 700：拆包后最大块是 echarts(657KB)，再出现 700KB+ 的
    // 单块就说明又有依赖被整体打包，构建时应立即告警而不是继续放行。
    chunkSizeWarningLimit: 700,
    // CSS 按路由拆分（2026-10 起）。
    //
    // 历史：曾合并成单 css 文件（466KB raw / 71KB gzip）以共享 gzip 字典，
    // 但首屏要等全站样式下载完才渲染，登录页被迫背 56% 用不到的 Element Plus
    // 组件样式与约百个页面的 SFC 样式。
    //
    // 试验数据（详见 scripts/check-bundle-size.mjs 注释）：
    //   拆分后 155 个 CSS / gzip 合计 107KB（+36KB 字典损失），
    //   首屏 CSS 71→13.9KB gzip（-57KB），总 gzip 1059.6KB。
    //   配合体积预算 1024→1100KB 上调后合入。
    cssCodeSplit: true,
    rollupOptions: {
      output: {
        // 手动分包：把大依赖拆成独立 chunk，利于浏览器缓存
        // mock 数据集中到 'mock-data' chunk，便于追踪大小（当前 gzip ~50KB，远低于 500KB 硬约束）
        manualChunks(id) {
          if (id.includes('node_modules/vue') || id.includes('node_modules/vue-router') || id.includes('node_modules/pinia')) return 'vue'
          if (id.includes('node_modules/echarts')) {
            // 把使用频次低、体积大的扩展组件（图拓扑/自定义绘制/柱状图/区域缩放/标题/标线）
            // 拆到 echarts-extra chunk，只有用到的页面才加载，普通折线图页减负。
            // 注意：echarts 的 lib 路径用小写（graph/custom/bar/dataZoom/markLine/title），
            // 而非 import 名（GraphChart 等）。
            if (id.includes('lib/chart/graph') || id.includes('lib/chart/custom') || id.includes('lib/chart/bar')
              || id.includes('lib/component/dataZoom') || id.includes('lib/component/markLine')
              || id.includes('lib/component/title') || id.includes('lib/component/marker')) return 'echarts-extra'
            return 'echarts'
          }
          // axios 单独拆 chunk：HTTP 基建层不随业务迭代失效缓存
          if (id.includes('node_modules/axios')) return 'http-vendor'
          // 注意：**不要**把 element-plus 整体固定成单一 chunk。
          // 曾用 `return 'element-plus'` 把所有 EP 模块捆成一块，而入口 main.ts
          // import 了 ElMessage，导致这块 600KB JS + 266KB CSS 被 modulepreload
          // 到首屏——登录页根本用不到表格、树、上传等组件。
          // 交给 Rollup 按动态路由切分：入口真正用到的进 index，其余进各页面
          // 共享的异步 chunk，首屏只下载实际渲染需要的组件与样式。
          //
          // el-icons 同样不固定 chunk：登录页只注册 5 个图标（main.ts 同步导入），
          // 其余 77 个在 utils/icons-extra.ts 中动态导入注册。去掉 manualChunks 后，
          // Rollup 会把 5 个登录图标放进主 chunk，77 个放进懒加载 chunk，首屏减负。
          if (id.includes('/src/mock/') || id.includes('\\src\\mock\\')) return 'mock-data'
        },
      },
    },
  },
})
