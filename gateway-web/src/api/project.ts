import { request } from './index'

/**
 * 工程文件信封（决策表 #8）：可移植的「三层子集」+ 自描述头。
 *
 * 与 `/api/config/export`（整份配置、含 web 凭据）不同，工程文件**不携带**
 * 本机 web 鉴权 / ntp / license / self_heal，便于在设备/现场之间搬运。
 */
export interface ProjectFile {
  /** 固定 "gateway-project"，导入端据此拒绝误传的普通 JSON */
  kind: string
  /** 工程文件自身 Schema 版本 */
  schema_version: number
  /** 导出时刻（Unix 秒），仅供追溯 */
  exported_at: number
  /** 总线层 */
  buses: Record<string, unknown>[]
  /** 北向层：mqtt / webhook / ... */
  northbound: Record<string, unknown>
  /** 变量映射层：points / devices / virtual_points / ... */
  mappings: Record<string, unknown>
}

/** 导出工程文件（总线 / 北向 / 变量映射三层）。默认脱敏凭据，导入时由后端还原。 */
export function exportProject(includeSecrets = false): Promise<ProjectFile> {
  return request<ProjectFile>({
    url: '/api/project/export',
    method: 'GET',
    params: includeSecrets ? { include_secrets: true } : undefined,
  })
}

/** 导入工程文件：仅覆盖三层，保留本机 web 鉴权 / ntp 等配置 */
export function importProject(project: ProjectFile): Promise<void> {
  return request<void>({ url: '/api/project/import', method: 'POST', data: project })
}
