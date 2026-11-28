import js from '@eslint/js'
import tseslint from 'typescript-eslint'
import vue from 'eslint-plugin-vue'
import prettier from 'eslint-config-prettier'
import globals from 'globals'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

// ── 行数预算（O-8）──
// 约定见 docs/architecture/frontend.md：单文件超过 ~800 行即视为巨型文件，必须拆分。
// 这里把约定变成 lint 断言：
//   - 新增/未登记文件超过预算 → warning。CI 用 `--max-warnings 0` 跑 lint，所以等于阻断。
//   - 存量超限文件登记在**冻结账本**里，并带**单文件硬上限**（超出即 error），
//     拆到 ≤800 行后请把对应行从账本删掉。
// 预算 500 → 800：500 行的阈值会把「正常但偏大」的组件也逼成巨型文件，
// 于是每次加功能都要先拆文件，拆分本身又制造新文件与新 PR —— 门禁从
// 「防失控」退化成「无限拆分」。后端 C24 用同一个 800 口径。
const LINE_BUDGET = 800

// 自动生成物：不纳入行数预算（生成物不靠人拆）。
const LINE_BUDGET_GENERATED = [
  // 自动生成，勿手改（vite alias 重定向 @element-plus/icons-vue 的产物）
  'src/utils/ep-icons-compact.ts',
]

// 冻结账本与后端共用一份（DD-58）：scripts/line_budget_frozen.txt
// 格式：<仓库相对路径><TAB><行数上限>，# 开头为注释。
// 前端条目以 `gateway-web/` 开头，这里转成 eslint 相对路径（src/...）。
const FROZEN_LEDGER = resolve(dirname(fileURLToPath(import.meta.url)), '../scripts/line_budget_frozen.txt')

function loadFrozenFrontendCaps() {
  const caps = new Map()
  if (!existsSync(FROZEN_LEDGER)) return caps
  const text = readFileSync(FROZEN_LEDGER, 'utf8')
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim()
    if (!trimmed || trimmed.startsWith('#')) continue
    const sep = trimmed.lastIndexOf('\t')
    if (sep <= 0) continue
    const rel = trimmed.slice(0, sep).trim()
    const cap = Number(trimmed.slice(sep + 1).trim())
    if (!Number.isInteger(cap)) continue
    if (!rel.startsWith('gateway-web/')) continue
    caps.set(rel.slice('gateway-web/'.length), cap)
  }
  return caps
}

const FROZEN_FRONTEND_CAPS = loadFrozenFrontendCaps()

export default tseslint.config(
  {
    ignores: [
      'node_modules/',
      'dist/',
      'coverage/',
      'src/api/generated/',
      '*.config.js',
      // Playwright E2E 产物
      'playwright-report/',
      'test-results/',
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...vue.configs['flat/essential'],
  {
    languageOptions: {
      globals: {
        ...globals.browser,
        ...globals.node,
      },
    },
    files: ['**/*.vue'],
    languageOptions: {
      parserOptions: {
        parser: tseslint.parser,
      },
    },
    rules: {
      'vue/multi-word-component-names': 'off',
    },
  },
  {
    rules: {
      // TS 已覆盖全局变量检查，关闭避免重复
      'no-undef': 'off',
      '@typescript-eslint/no-explicit-any': 'warn',
      '@typescript-eslint/no-unused-vars': ['warn', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      // 业务代码禁止直接 console.*，统一走 @/utils/logger（分级输出 + 生产静默）
      'no-console': 'error',
      '@typescript-eslint/no-empty-object-type': 'off',
      'vue/no-ref-as-operand': 'warn',
    },
  },
  // 日志唯一出口：logger.ts 内部必须直接写 console 才能实现分级 + 作用域封装
  {
    files: ['src/utils/logger.ts'],
    rules: {
      'no-console': 'off',
    },
  },
  // #17: 禁止 composable / utils 文件中使用模块级 ref/reactive 创建全局单例
  // 新增模块级 ref 会触发 warning，开发者应使用 Pinia store（跨页面）或将 ref 移入函数内（页面级）
  // 有意的单例用 // eslint-disable-next-line no-restricted-syntax 标注
  {
    files: ['src/composables/**/*.ts', 'src/utils/**/*.ts'],
    rules: {
      'no-restricted-syntax': ['warn',
        {
          selector: 'Program > VariableDeclaration > VariableDeclarator > CallExpression[callee.name=\'ref\']',
          message: 'Module-level ref() in composable creates a global singleton. Use Pinia store for cross-page state, or move inside function. If intentional, add // eslint-disable-next-line no-restricted-syntax',
        },
        {
          selector: 'Program > VariableDeclaration > VariableDeclarator > CallExpression[callee.name=\'reactive\']',
          message: 'Module-level reactive() in composable creates a global singleton. Use Pinia store for cross-page state, or move inside function. If intentional, add // eslint-disable-next-line no-restricted-syntax',
        },
      ],
    },
  },
  // E2E 测试、服务夹具与构建/报告脚本、单元测试：console 是正当的日志/断言辅助，放宽 no-console
  {
    files: ['e2e/**/*.ts', 'playwright.config.ts', 'scripts/**/*.mjs', 'src/**/*.spec.ts'],
    rules: {
      'no-console': 'off',
    },
  },
  // 行数预算：业务源码纳入检查；自动生成物与已登记文件除外
  {
    files: ['src/**/*.{ts,vue}'],
    ignores: [...LINE_BUDGET_GENERATED, ...FROZEN_FRONTEND_CAPS.keys()],
    rules: {
      'max-lines': ['warn', { max: LINE_BUDGET }],
    },
  },
  // 已登记文件按冻结账本里的**单文件硬上限**判定（超出即 error，存量不得悄悄长）。
  // 与后端 C24 的棘轮口径一致：账本改动会进 review，不会被静默绕过。
  ...[...FROZEN_FRONTEND_CAPS.entries()].map(([file, cap]) => ({
    files: [file],
    rules: {
      'max-lines': ['error', { max: cap }],
    },
  })),
  {
    files: ['src/**/*.spec.ts', 'src/**/*.test.ts', 'src/test/**/*.ts'],
    rules: {
      'max-lines': 'off',
    },
  },
  prettier,
)
