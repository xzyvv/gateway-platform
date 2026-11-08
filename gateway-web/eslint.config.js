import js from '@eslint/js'
import tseslint from 'typescript-eslint'
import vue from 'eslint-plugin-vue'
import prettier from 'eslint-config-prettier'
import globals from 'globals'

// ── 行数预算（O-8）──
// 约定见 docs/architecture/frontend.md：单文件超过 ~500 行即视为巨型文件，必须拆分。
// 这里把约定变成 lint 断言：
//   - 新增/未登记文件超过预算 → warning。CI 用 `--max-warnings 0` 跑 lint，所以等于阻断。
//   - 存量超限文件登记在下方清单里（豁免），不阻断当前 CI；
//     拆到 ≤500 行后**请把对应行删掉**，否则它会一直享受豁免。
const LINE_BUDGET = 500
const LINE_BUDGET_EXEMPT = [
  // 自动生成，勿手改（vite alias 重定向 @element-plus/icons-vue 的产物）
  'src/utils/ep-icons-compact.ts',
  // ── 存量超限（债务清单：只许减少，不许新增）──
  'src/utils/echarts.ts',
  'src/views/acquisition/console/DebugWizard.vue',
  'src/views/devices/DeviceList.vue',
  'src/views/ops/delivery/selfCheck.ts',
  'src/views/points/PointList.vue',
]

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
  // #17: 禁止 composable 文件中使用模块级 ref/reactive 创建全局单例
  // 新增模块级 ref 会触发 warning，开发者应使用 Pinia store（跨页面）或将 ref 移入函数内（页面级）
  // 有意的单例用 // eslint-disable-next-line no-restricted-syntax 标注
  {
    files: ['src/composables/**/*.ts'],
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
  // 行数预算：业务源码纳入检查，测试文件与豁免清单除外
  {
    files: ['src/**/*.{ts,vue}'],
    ignores: LINE_BUDGET_EXEMPT,
    rules: {
      'max-lines': ['warn', { max: LINE_BUDGET }],
    },
  },
  {
    files: ['src/**/*.spec.ts', 'src/**/*.test.ts', 'src/test/**/*.ts'],
    rules: {
      'max-lines': 'off',
    },
  },
  prettier,
)
