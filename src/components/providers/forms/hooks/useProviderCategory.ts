import { useState } from "react";
import type { ProviderCategory } from "@/types";

interface UseProviderCategoryProps {
  isEditMode: boolean;
  initialCategory?: ProviderCategory;
}

/**
 * 管理供应商类别状态
 */
export function useProviderCategory({
  isEditMode,
  initialCategory,
}: UseProviderCategoryProps) {
  const [category, setCategory] = useState<ProviderCategory | undefined>(
    // 编辑模式：使用 initialCategory，新建模式：custom
    isEditMode ? initialCategory : "custom" as ProviderCategory | undefined,
  );

  return { category, setCategory };
}