/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 生产环境打开降级日志输出（logger.ts 默认静默） */
  readonly VITE_LOG_VERBOSE?: string
}

declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, unknown>
  export default component
}

/** Element Plus 的按需样式入口是纯 .mjs 且不提供类型声明，显式 import 时按无类型模块处理 */
declare module 'element-plus/es/components/table-v2/style/css'
declare module 'element-plus/es/components/pagination/style/css'
declare module 'element-plus/es/components/badge/style/css'
