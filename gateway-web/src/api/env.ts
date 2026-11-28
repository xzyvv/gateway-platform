/**
 * 运行环境检测（api 层入口）。
 *
 * 实现已下沉到 utils/platform.ts（DD-86）；此处保留再导出，既有 `@/api` 调用方不变。
 */
export { isTauri } from '@/utils/platform'
