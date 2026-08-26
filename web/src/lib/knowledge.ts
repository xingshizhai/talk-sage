import type { AppConfig } from "./api";

/** 读已保存的知识库开关与文件夹；未配置时关闭、路径为空。 */
export function knowledgeBaseSettings(
  config: AppConfig | null | undefined,
): { enabled: boolean; folder: string } {
  return {
    enabled: config?.knowledge_base?.enabled ?? false,
    folder: config?.knowledge_base?.folder ?? "",
  };
}
