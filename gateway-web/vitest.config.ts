import { defineConfig } from 'vitest/config'
import { fileURLToPath, URL } from 'node:url'
import vue from '@vitejs/plugin-vue'
import AutoImport from 'unplugin-auto-import/vite'
import Components from 'unplugin-vue-components/vite'
import { ElementPlusResolver } from 'unplugin-vue-components/resolvers'

/**
 * 覆盖率统计范围：**默认全量口径**（src 下全部 ts/vue 源码）。
 *
 * 历史：v8 provider 默认只统计「被测试 import 过的文件」，新增一个没有任何测试的
 * 页面不会拉低覆盖率，门禁等于对盲区免疫；此前用 COVERAGE_FULL=1 做选择性全量。
 *
 * 2026-11-03（C1.4）：已用 COVERAGE_FULL=1 跑出一份全量基线（coverage-summary.json），
 * 按「实测下取整、留约 3pp 波动余量」重定下方分目录阈值，并移除开关、默认全量统计。
 * 新增无测试文件从此会拉低对应目录覆盖率，不再免疫门禁。
 *
 * 重定方法（后续需要上调时）：`npx vitest run --coverage` 生成
 * coverage/coverage-summary.json，按目录聚合 covered/total 后更新 thresholds。
 */

export default defineConfig({
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      '@element-plus/icons-vue': fileURLToPath(new URL('./src/utils/ep-icons-compact.ts', import.meta.url)),
    },
  },
  plugins: [
    vue(),
    AutoImport({ resolvers: [ElementPlusResolver()] }),
    Components({ resolvers: [ElementPlusResolver()] }),
  ],
  test: {
    environment: 'jsdom',
    globals: true,
    // 单例超时：默认 5s 在 CI 上不够（4 worker + v8 覆盖率插桩，实测 DataTable /
    // RuntimeConfig / BatchReadTest 三处重型 mount 恰好卡在 5s 边界超时，本地同版本全绿）。
    // 放宽到 15s：只影响真失败时的等待时间，不影响通过的用例耗时。
    testTimeout: 15_000,
    // vmThreads 池：worker 复用 + VM context 保持逐文件隔离，显著降低 jsdom
    // 环境重建开销（实测 jsdom 创建曾占总时长 57%）。不用 isolate:false
    // （跨文件共享环境）——本仓测试大量依赖 localStorage/全局桩，共享会互相污染。
    pool: 'vmThreads',
    // 限制 worker 并发：默认按 CPU 核数拉满，本机在多核下会因句柄/内存耗尽
    // 出现 "Failed to start forks worker"（部分用例文件未能启动，静默漏跑）。
    // 固定上限以兼顾稳定与速度，可按机器再调（均需 ≥1）。
    // 注意：Vitest 4 起 `test.poolOptions.forks.maxForks/minForks` 已移除并被静默忽略，
    // 必须用顶层 maxWorkers/minWorkers；写成旧字段不会报错但**完全不生效**。
    maxWorkers: 4,
    minWorkers: 1,
    // ── 2026-11-03 flaky 排查结论（别再重复追这个坑）──
    // 现象：全量 vitest 偶发「每次换一个文件失败 1 条」，单独跑该文件必过。
    // 结论：不是跨文件状态污染——isolate 默认开启，setup.ts 每例重置 Pinia 与共享调度器；
    //       而是**文件正在被改写时并发跑了全量测试**：断言基于改写前的内容、组件已被改写，
    //       叠加另一个 vitest 进程抢 CPU，期望与实现错位（失败文件每次都不同，正是特征）。
    // 证据：写入停止后连续 5 次全量（215 文件 / 1978 用例）+ 一次 coverage 模式全绿。
    // 规矩：改源码/批量生成用例期间不要跑全量；CI 是只读检出，不存在该问题。
    css: false,
    // 跨运行持久化 transform 结果（默认 node_modules/.vitest-cache，已随 node_modules 被 gitignore）。
    // 本仓 element-plus 走 server.deps.inline，每次冷跑 transform 占比高；开启后第二次起显著省时。
    fsModuleCache: true,
    server: {
      deps: {
        inline: ['element-plus'],
      },
    },
    setupFiles: ['./src/test/setup.ts'],
    // 测试约定：co-locate（.spec.ts 与被测源文件同目录），不使用 __tests__/ 子目录。
    // 例：useFoo.ts + useFoo.spec.ts；MetricsDashboard.vue + MetricsDashboard.spec.ts。
    include: ['src/**/*.{test,spec}.{ts,js}'],
    coverage: {
      provider: 'v8',
      // 全量口径：始终统计 src 下全部源码（见文件顶部说明）。
      include: ['src/**/*.{ts,vue}'],
      // json-summary：机器可读的汇总（coverage/coverage-summary.json），
      // 供 CI 归档与「本次 vs 基线」的覆盖率差值对比；text/html 仅供人读。
      reporter: ['text', 'html', 'json-summary'],
      exclude: [
        'src/api/generated/**',
        'src/**/*.css',
        'src/**/*.d.ts',
        'src/test/setup.ts',
        '*.config.ts',
        '*.config.js',
      ],
      // 分目录阈值：2026-11-03 按**全量口径**实测（covered/total 聚合）下取整、
      // 留约 3pp 波动余量设定，定位是各目录的「防回落地板」，补测试时只许上调、
      // 不许下调。分组按一级目录互斥；未匹配任何 glob 的文件（如 src 根下与
      // src/app 下文件）适用顶层兜底阈值。
      // 未列出的指标（如 router/types 的 functions/branches 当前无意义）表示该组不检查。
      //
      // 全量实测基线（2026-11-03，D1 第五批后，1978 tests / 215 files）：
      //   total        lines 66.22 / funcs 59.87 / branches 56.64 / stmts 63.45
      //   api          lines 74.80 / funcs 67.55 / branches 75.00 / stmts 74.12
      //   components   lines 75.25 / funcs 70.29 / branches 67.04 / stmts 73.38
      //   composables  lines 64.86 / funcs 62.49 / branches 48.65 / stmts 61.58
      //   mock         lines 21.74 / funcs 35.00 / branches  9.09 / stmts 24.32
      //   router       lines 40.32 / branches 42.11 / stmts 40.58
      //   stores       lines 72.73 / funcs 79.38 / branches 70.32 / stmts 72.77
      //   types        lines/statements 100
      //   utils        lines 79.72 / funcs 73.73 / branches 67.38 / stmts 77.52
      //   views        lines 61.75 / funcs 56.64 / branches 54.68 / stmts 58.98
      //   （views 33.78 → 37.44 → 47.33 → 52.95 → 58.33 → 61.75：D1 第一批补采集调优/点位读写/健康报告，
      //     第二批补交付验收/现场调试向导/通道转发/设备分组/实时监视五个 A 档页面，
      //     第三批补总线配置/配置导入导出/片段导入/读取性能测试/设备批量导入/批量写入，
      //     第四批补步骤序列/虚拟点位公式/总线负载/点位批量导入/蜂窝网络/设备健康评分，
      //     第五批补配置版本历史/配置体检/地址偏移/地址换算/设备模板/数据质量概览）
      thresholds: {
        lines: 63,
        functions: 56,
        branches: 53,
        statements: 60,
        '**/src/api/**': { lines: 71, functions: 64, branches: 71, statements: 70 },
        '**/src/components/**': { lines: 72, functions: 67, branches: 64, statements: 70 },
        '**/src/composables/**': { lines: 61, functions: 59, branches: 45, statements: 58 },
        '**/src/mock/**': { lines: 18, functions: 32, branches: 6, statements: 21 },
        '**/src/router/**': { lines: 37, statements: 37 },
        '**/src/stores/**': { lines: 69, functions: 76, branches: 67, statements: 69 },
        '**/src/types/**': { lines: 97, statements: 97 },
        '**/src/utils/**': { lines: 76, functions: 71, branches: 64, statements: 74 },
        '**/src/views/**': { lines: 58, functions: 53, branches: 51, statements: 55 },
      },
    },
  },
})
